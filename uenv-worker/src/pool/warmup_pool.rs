use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::plugin::host::PluginHost;
use crate::plugin::instance::PluginInstance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceStatus {
    Creating,
    Warm,
    Active,
    Idle,
    Cooling,
    Evicting,
    Destroyed,
}

#[derive(Debug, Clone, Copy)]
pub struct WarmupPoolConfig {
    pub warmup_size: u32,
    pub max_idle_time_secs: u32,
    pub cool_timeout_secs: u32,
    pub max_episode_count: u32,
}

#[derive(Debug, Clone)]
pub struct WarmLease {
    pub instance_id: String,
    pub env_type: String,
    pub warmup_hit: bool,
}

#[derive(Debug, Clone)]
struct TrackedInstance {
    instance: PluginInstance,
    status: InstanceStatus,
    episode_count: u32,
    last_used_unix_secs: u64,
}

#[derive(Default)]
struct PoolState {
    tracked: HashMap<String, TrackedInstance>,
    warm_queues: HashMap<String, VecDeque<String>>,
    active: HashSet<String>,
}

#[derive(Clone)]
pub struct WarmupPool {
    plugin_host: PluginHost,
    cfg: WarmupPoolConfig,
    state: std::sync::Arc<tokio::sync::Mutex<PoolState>>,
}

impl WarmupPool {
    pub fn new(plugin_host: PluginHost, cfg: WarmupPoolConfig) -> Self {
        Self {
            plugin_host,
            cfg,
            state: std::sync::Arc::new(tokio::sync::Mutex::new(PoolState::default())),
        }
    }

    pub async fn prewarm(
        &self,
        env_types: &[String],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        for env_type in env_types {
            self.fill_pool(env_type).await?;
        }
        Ok(())
    }

    pub async fn acquire(
        &self,
        env_type: &str,
    ) -> Result<WarmLease, Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let candidate = {
                let mut state = self.state.lock().await;
                state
                    .warm_queues
                    .entry(env_type.to_string())
                    .or_default()
                    .pop_front()
            };

            if let Some(instance_id) = candidate {
                if self.plugin_host.health_check(&instance_id).await.unwrap_or(false) {
                    let mut state = self.state.lock().await;
                    if state.active.contains(&instance_id) {
                        return Err(format!("double allocation detected for {instance_id}").into());
                    }
                    state.active.insert(instance_id.clone());
                    if let Some(tracked) = state.tracked.get_mut(&instance_id) {
                        tracked.status = InstanceStatus::Active;
                    }
                    drop(state);
                    self.fill_pool(env_type).await?;
                    return Ok(WarmLease {
                        instance_id,
                        env_type: env_type.to_string(),
                        warmup_hit: true,
                    });
                }
                self.destroy_instance(&instance_id).await;
                self.fill_pool(env_type).await?;
                continue;
            }

            let instance = self.plugin_host.spawn(env_type).await?;
            let instance_id = instance.instance_id.clone();
            let now = unix_now_secs();
            let mut state = self.state.lock().await;
            state.tracked.insert(
                instance_id.clone(),
                TrackedInstance {
                    instance,
                    status: InstanceStatus::Active,
                    episode_count: 0,
                    last_used_unix_secs: now,
                },
            );
            if state.active.contains(&instance_id) {
                return Err(format!("double allocation detected for {instance_id}").into());
            }
            state.active.insert(instance_id.clone());
            drop(state);
            self.fill_pool(env_type).await?;
            return Ok(WarmLease {
                instance_id,
                env_type: env_type.to_string(),
                warmup_hit: false,
            });
        }
    }

    pub async fn release(&self, lease: WarmLease) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let should_destroy = {
            let mut state = self.state.lock().await;
            if !state.active.remove(&lease.instance_id) {
                return Err(format!("instance {} is not active", lease.instance_id).into());
            }
            let Some(tracked) = state.tracked.get_mut(&lease.instance_id) else {
                return Ok(());
            };
            tracked.episode_count += 1;
            tracked.last_used_unix_secs = unix_now_secs();
            tracked.status = InstanceStatus::Cooling;
            tracked.episode_count >= self.cfg.max_episode_count.max(1)
        };

        if should_destroy {
            self.destroy_instance(&lease.instance_id).await;
            self.fill_pool(&lease.env_type).await?;
            return Ok(());
        }

        let reset_ok = self
            .plugin_host
            .reset(&lease.instance_id, None)
            .await
            .map(|_| true)
            .unwrap_or(false);
        let health_ok = self
            .plugin_host
            .health_check(&lease.instance_id)
            .await
            .unwrap_or(false);

        if !(reset_ok && health_ok) {
            self.destroy_instance(&lease.instance_id).await;
            self.fill_pool(&lease.env_type).await?;
            return Ok(());
        }

        {
            let mut state = self.state.lock().await;
            if let Some(tracked) = state.tracked.get_mut(&lease.instance_id) {
                tracked.status = InstanceStatus::Warm;
                tracked.last_used_unix_secs = unix_now_secs();
            }
            state
                .warm_queues
                .entry(lease.env_type.clone())
                .or_default()
                .push_back(lease.instance_id.clone());
        }

        self.evict_idle(&lease.env_type).await;
        self.fill_pool(&lease.env_type).await?;
        Ok(())
    }

    pub async fn status_counts(&self) -> HashMap<&'static str, u64> {
        let state = self.state.lock().await;
        let mut out: HashMap<&'static str, u64> = HashMap::new();
        for tracked in state.tracked.values() {
            let key = match tracked.status {
                InstanceStatus::Creating => "creating",
                InstanceStatus::Warm => "warm",
                InstanceStatus::Active => "active",
                InstanceStatus::Idle => "idle",
                InstanceStatus::Cooling => "cooling",
                InstanceStatus::Evicting => "evicting",
                InstanceStatus::Destroyed => "destroyed",
            };
            *out.entry(key).or_insert(0) += 1;
        }
        out
    }

    async fn fill_pool(
        &self,
        env_type: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let target = self.cfg.warmup_size as usize;
        loop {
            let warm_size = {
                let state = self.state.lock().await;
                state.warm_queues.get(env_type).map(|q| q.len()).unwrap_or(0)
            };
            if warm_size >= target {
                return Ok(());
            }
            let instance = self.plugin_host.spawn(env_type).await?;
            let instance_id = instance.instance_id.clone();
            let now = unix_now_secs();
            let mut state = self.state.lock().await;
            state.tracked.insert(
                instance_id.clone(),
                TrackedInstance {
                    instance,
                    status: InstanceStatus::Warm,
                    episode_count: 0,
                    last_used_unix_secs: now,
                },
            );
            state
                .warm_queues
                .entry(env_type.to_string())
                .or_default()
                .push_back(instance_id);
        }
    }

    async fn evict_idle(&self, env_type: &str) {
        let now = unix_now_secs();
        let max_idle = self.cfg.max_idle_time_secs as u64;
        let cool_timeout = self.cfg.cool_timeout_secs as u64;
        let evict_candidates = {
            let state = self.state.lock().await;
            let mut ids = Vec::new();
            if let Some(queue) = state.warm_queues.get(env_type) {
                for id in queue {
                    if let Some(inst) = state.tracked.get(id) {
                        let idle_for = now.saturating_sub(inst.last_used_unix_secs);
                        if idle_for > max_idle.max(cool_timeout) {
                            ids.push(id.clone());
                        }
                    }
                }
            }
            ids
        };
        for id in evict_candidates {
            self.destroy_instance(&id).await;
        }
    }

    async fn destroy_instance(&self, instance_id: &str) {
        let env_type = {
            let state = self.state.lock().await;
            state
                .tracked
                .get(instance_id)
                .map(|tracked| tracked.instance.env_type.clone())
        };
        if let Some(env_type) = env_type {
            {
                let mut state = self.state.lock().await;
                if let Some(tracked) = state.tracked.get_mut(instance_id) {
                    tracked.status = InstanceStatus::Evicting;
                }
            }
            let _ = self.plugin_host.close(instance_id).await;
            let mut state = self.state.lock().await;
            if let Some(queue) = state.warm_queues.get_mut(&env_type) {
                queue.retain(|id| id != instance_id);
            }
            state.active.remove(instance_id);
            state.tracked.remove(instance_id);
        }
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
