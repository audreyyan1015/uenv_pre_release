//! ChainState 归并：lifecycle + workflow/tree 投影。

use super::event::{
    now_ms, resolve_training_run_id, ChainState, Disposition, EpisodeView, EventCursor,
    ObservabilityEvent, StateDelta, TreeNode, WorkerView,
};
use serde_json::json;
use std::collections::HashMap;

#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct EntityVersion {
    pub confirmed_seq: u64,
    pub event_seq: u64,
    pub source_id: String,
}

#[derive(Debug, Default)]
pub struct RunLifecycle {
    pub run_state: String,
    pub closed: bool,
}

#[derive(Debug)]
pub struct MergeEngine {
    pub runs: HashMap<String, ChainState>,
    #[allow(dead_code)]
    versions: HashMap<String, EntityVersion>,
    lifecycles: HashMap<String, RunLifecycle>,
    /// source_id -> confirmed seq
    source_seq: HashMap<String, u64>,
}

impl Default for MergeEngine {
    fn default() -> Self {
        Self {
            runs: HashMap::new(),
            versions: HashMap::new(),
            lifecycles: HashMap::new(),
            source_seq: HashMap::new(),
        }
    }
}

pub struct MergeOutcome {
    pub disposition: Disposition,
    pub training_run_id: String,
    pub delta: Option<StateDelta>,
    pub full_hint: bool,
}

impl MergeEngine {
    pub fn get_or_create(&mut self, run_id: &str) -> &mut ChainState {
        self.runs
            .entry(run_id.to_string())
            .or_insert_with(|| ChainState::empty(run_id))
    }

    pub fn get(&self, run_id: &str) -> Option<&ChainState> {
        self.runs.get(run_id)
    }

    pub fn apply(&mut self, ev: &ObservabilityEvent, ingest_ts: i64) -> MergeOutcome {
        let training_run_id =
            resolve_training_run_id(ev.training_run_id.as_deref().filter(|s| !s.is_empty()));

        let life = self
            .lifecycles
            .entry(training_run_id.clone())
            .or_insert_with(|| RunLifecycle {
                run_state: "PENDING".into(),
                closed: false,
            });

        if life.closed
            && !matches!(
                ev.event_type.as_str(),
                "RUN_STARTED" | "RUN_STOPPED" | "RUN_CLOSED"
            )
        {
            return MergeOutcome {
                disposition: Disposition::RejectedClosed,
                training_run_id,
                delta: None,
                full_hint: false,
            };
        }

        if let Some(prev) = self.source_seq.get(&ev.source_id).copied() {
            if ev.seq < prev {
                return MergeOutcome {
                    disposition: Disposition::SeqStale,
                    training_run_id,
                    delta: None,
                    full_hint: false,
                };
            }
            if ev.seq == prev {
                return MergeOutcome {
                    disposition: Disposition::Duplicate,
                    training_run_id,
                    delta: None,
                    full_hint: false,
                };
            }
        }
        self.source_seq.insert(ev.source_id.clone(), ev.seq);

        {
            let state = self.get_or_create(&training_run_id);
            state.global_event_seq += 1;
            state.updated_at = ingest_ts;
            state.cursor = EventCursor {
                last_event_id: ev.event_id.clone(),
                last_source_id: ev.source_id.clone(),
                last_seq: ev.seq,
                last_ingest_ts: ingest_ts,
            };
        }

        let event_seq = {
            let state = self.runs.get(&training_run_id).unwrap();
            state.global_event_seq
        };

        let delta = self.project_event(&training_run_id, ev, ingest_ts, event_seq);

        MergeOutcome {
            disposition: Disposition::Accepted,
            training_run_id,
            delta,
            full_hint: matches!(ev.event_type.as_str(), "RUN_STARTED" | "RUN_CLOSED"),
        }
    }

    fn project_event(
        &mut self,
        run_id: &str,
        ev: &ObservabilityEvent,
        ingest_ts: i64,
        event_seq: u64,
    ) -> Option<StateDelta> {
        match ev.event_type.as_str() {
            "RUN_STARTED" => {
                if let Some(life) = self.lifecycles.get_mut(run_id) {
                    life.run_state = "RUNNING".into();
                    life.closed = false;
                }
                let state = self.get_or_create(run_id);
                state.run_state = "RUNNING".into();
                set_run_tree_status(state, "ACTIVE");
                set_workflow_active(state, "submit", "ACTIVE", ev);
                Some(run_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "RUN_STOPPED" => {
                if let Some(life) = self.lifecycles.get_mut(run_id) {
                    life.run_state = "STOPPING".into();
                }
                let state = self.get_or_create(run_id);
                state.run_state = "STOPPING".into();
                set_run_tree_status(state, "ACTIVE");
                Some(run_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "RUN_CLOSED" => {
                if let Some(life) = self.lifecycles.get_mut(run_id) {
                    life.run_state = "CLOSED".into();
                    life.closed = true;
                }
                let state = self.get_or_create(run_id);
                state.run_state = "CLOSED".into();
                set_run_tree_status(state, "CLOSED");
                set_workflow_stage_status(state, "done", "DONE");
                Some(run_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "EPISODE_SUBMITTED" => {
                let ep_id = ev.episode_id.clone().unwrap_or_else(|| ev.entity_id.clone());
                {
                    let state = self.get_or_create(run_id);
                    state.episodes.insert(
                        ep_id.clone(),
                        EpisodeView {
                            episode_id: ep_id.clone(),
                            correlation_id: ev.correlation_id.clone(),
                            attempt_id: ev.attempt_id.unwrap_or(1),
                            worker_id: ev.worker_id.clone().unwrap_or_default(),
                            step_index: 0,
                            status: "ACTIVE".into(),
                            event_seq,
                            last_source_ts: ev.source_ts,
                            trajectory_id: None,
                        },
                    );
                    set_workflow_active(state, "submit", "DONE", ev);
                    set_workflow_active(state, "dispatch", "ACTIVE", ev);
                    bump_workflow_stage_count(state, "submit", &ep_id);
                    upsert_tree_episode(state, &ep_id, ev, "ACTIVE");
                }
                let state = self.runs.get(run_id).unwrap();
                Some(chain_patch_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "EPISODE_DISPATCHED" | "ATTEMPT_STARTED" => {
                let ep_id = ev.episode_id.clone().unwrap_or_else(|| ev.entity_id.clone());
                {
                    let state = self.get_or_create(run_id);
                    if let Some(ep) = state.episodes.get_mut(&ep_id) {
                        ep.attempt_id = ev.attempt_id.unwrap_or(ep.attempt_id);
                        if let Some(w) = &ev.worker_id {
                            ep.worker_id = w.clone();
                        }
                        ep.status = "ACTIVE".into();
                        ep.event_seq = event_seq;
                        ep.last_source_ts = ev.source_ts;
                    }
                    if let Some(wid) = &ev.worker_id {
                        let w = state.workers.entry(wid.clone()).or_insert_with(|| WorkerView {
                            worker_id: wid.clone(),
                            status: "ACTIVE".into(),
                            ..Default::default()
                        });
                        if !w.active_episodes.contains(&ep_id) {
                            w.active_episodes.push(ep_id.clone());
                        }
                        upsert_tree_worker(state, wid, "ACTIVE");
                    }
                    set_workflow_active(state, "dispatch", "DONE", ev);
                    set_workflow_active(state, "execute", "ACTIVE", ev);
                    bump_workflow_stage_count(state, "dispatch", &ep_id);
                    upsert_tree_episode(state, &ep_id, ev, "ACTIVE");
                }
                let state = self.runs.get(run_id).unwrap();
                Some(chain_patch_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "STEP_STARTED" | "STEP_COMPLETE" => {
                let ep_id = ev.episode_id.clone().unwrap_or_else(|| ev.entity_id.clone());
                let step = ev.step_index.unwrap_or(0);
                {
                    let state = self.get_or_create(run_id);
                    if let Some(ep) = state.episodes.get_mut(&ep_id) {
                        ep.step_index = step;
                        ep.event_seq = event_seq;
                        ep.last_source_ts = ev.source_ts;
                        ep.status = "ACTIVE".into();
                    }
                    set_workflow_active(state, "execute", "ACTIVE", ev);
                    bump_workflow_stage_count(state, "execute", &ep_id);
                    upsert_tree_step(state, &ep_id, step, ev);
                }
                let state = self.runs.get(run_id).unwrap();
                Some(chain_patch_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "EPISODE_COMPLETED" | "EPISODE_FAILED" | "EPISODE_CLOSED" => {
                let ep_id = ev.episode_id.clone().unwrap_or_else(|| ev.entity_id.clone());
                let status = match ev.event_type.as_str() {
                    "EPISODE_FAILED" => "FAILED",
                    _ => "DONE",
                };
                {
                    let state = self.get_or_create(run_id);
                    if let Some(ep) = state.episodes.get_mut(&ep_id) {
                        ep.status = status.into();
                        ep.event_seq = event_seq;
                        ep.last_source_ts = ev.source_ts;
                        if let Some(payload) = &ev.payload {
                            if let Some(tid) = payload.get("trajectory_id").and_then(|v| v.as_str())
                            {
                                ep.trajectory_id = Some(tid.to_string());
                            }
                        }
                    }
                    if let Some(wid) = state
                        .episodes
                        .get(&ep_id)
                        .map(|e| e.worker_id.clone())
                        .filter(|s| !s.is_empty())
                    {
                        if let Some(w) = state.workers.get_mut(&wid) {
                            w.active_episodes.retain(|id| id != &ep_id);
                        }
                    }
                    set_workflow_active(state, "execute", "DONE", ev);
                    set_workflow_active(state, "report", "DONE", ev);
                    set_workflow_active(state, "done", status, ev);
                    // EPISODE_FAILED 同样计入 report 与终态（done）；COMPLETED/FAILED 之后的
                    // CLOSED 事件因 distinct 去重不会重复计数。
                    bump_workflow_stage_count(state, "report", &ep_id);
                    bump_workflow_stage_count(state, "done", &ep_id);
                    upsert_tree_episode(state, &ep_id, ev, status);
                    close_tree_steps_for_episode(state, &ep_id, status);
                }
                let state = self.runs.get(run_id).unwrap();
                Some(chain_patch_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "ATTEMPT_CLOSED" => {
                let ep_id = ev.episode_id.clone().unwrap_or_else(|| ev.entity_id.clone());
                {
                    let state = self.get_or_create(run_id);
                    if let Some(ep) = state.episodes.get_mut(&ep_id) {
                        ep.event_seq = event_seq;
                        ep.last_source_ts = ev.source_ts;
                    }
                }
                let state = self.runs.get(run_id).unwrap();
                Some(chain_patch_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            "WORKER_REGISTERED" | "WORKER_HEARTBEAT" => {
                let wid = ev
                    .worker_id
                    .clone()
                    .unwrap_or_else(|| ev.entity_id.clone());
                let is_heartbeat = ev.event_type == "WORKER_HEARTBEAT";
                {
                    let state = self.get_or_create(run_id);
                    let w = state.workers.entry(wid.clone()).or_insert_with(|| WorkerView {
                        worker_id: wid.clone(),
                        ..Default::default()
                    });
                    w.last_heartbeat_ts = ev.source_ts;
                    w.status = if ev.event_type == "WORKER_REGISTERED" {
                        "ACTIVE".into()
                    } else {
                        w.status.clone().or_default_active()
                    };
                    upsert_tree_worker(state, &wid, "ACTIVE");
                }
                // 心跳只更新内存态，不推 SSE，避免高频刷屏（规划：心跳不驱动 UI 主更新）。
                if is_heartbeat {
                    return None;
                }
                let state = self.runs.get(run_id).unwrap();
                Some(chain_patch_delta(run_id, event_seq, ingest_ts, ev, state))
            }
            _ => {
                let state = self.get_or_create(run_id);
                Some(StateDelta {
                    training_run_id: run_id.to_string(),
                    event_seq,
                    entity_key: "run".into(),
                    patch: json!({ "updated_at": ingest_ts }),
                    source_ts: ev.source_ts,
                    ingest_ts,
                    cursor: state.cursor.clone(),
                })
            }
        }
    }
}

trait OrActive {
    fn or_default_active(self) -> String;
}
impl OrActive for String {
    fn or_default_active(self) -> String {
        if self.is_empty() {
            "ACTIVE".into()
        } else {
            self
        }
    }
}

fn run_delta(
    run_id: &str,
    event_seq: u64,
    ingest_ts: i64,
    ev: &ObservabilityEvent,
    state: &ChainState,
) -> StateDelta {
    StateDelta {
        training_run_id: run_id.to_string(),
        event_seq,
        entity_key: "run".into(),
        patch: json!({
            "run_state": state.run_state,
            "workflow": state.workflow,
            "tree": state.tree,
            "episodes": state.episodes,
            "workers": state.workers,
            "updated_at": state.updated_at,
        }),
        source_ts: ev.source_ts,
        ingest_ts,
        cursor: state.cursor.clone(),
    }
}

/// 复合增量：统一用 entity_key=`run`，patch 为 ChainState 顶层字段。
/// 前端按 entity_key 分派时，episode:/worker:/step: 只会 deepMerge 到子对象，
/// 若把 workflow/tree 塞进子 patch 会丢更新或污染 EpisodeView。
fn chain_patch_delta(
    run_id: &str,
    event_seq: u64,
    ingest_ts: i64,
    ev: &ObservabilityEvent,
    state: &ChainState,
) -> StateDelta {
    run_delta(run_id, event_seq, ingest_ts, ev, state)
}

fn set_run_tree_status(state: &mut ChainState, status: &str) {
    let node_id = format!("run:{}", state.training_run_id);
    if let Some(n) = state.tree.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.status = status.into();
    }
}

/// 按口径（Docs/adapter/20260727 §2.5）更新 workflow 阶段的「关联实体」计数：
/// 同一 episode 在同一阶段只计一次（distinct episode，而不是事件次数），
/// 计数写入 node.payload_summary = {"count": N} 供前端读取。
/// retry 导致的同 episode 重复 dispatch / 多 step 的 execute 均不重复计数；
/// 没有 episode_id 的纯 run 事件（ep_id 为空）不影响计数。
fn bump_workflow_stage_count(state: &mut ChainState, stage: &str, ep_id: &str) {
    if ep_id.is_empty() {
        return;
    }
    let seen = state
        .stage_seen_episodes
        .entry(stage.to_string())
        .or_default();
    if !seen.insert(ep_id.to_string()) {
        return;
    }
    let count = seen.len();
    if let Some(n) = state
        .workflow
        .nodes
        .iter_mut()
        .find(|n| n.node_id == stage)
    {
        n.payload_summary = json!({ "count": count });
    }
}

fn set_workflow_active(state: &mut ChainState, node_id: &str, status: &str, ev: &ObservabilityEvent) {
    set_workflow_stage_status(state, node_id, status);
    if let Some(n) = state.workflow.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.correlation_id = ev.correlation_id.clone();
        if let Some(ep) = &ev.episode_id {
            n.episode_id = ep.clone();
        }
        n.source_ts = ev.source_ts;
    }
    if status == "ACTIVE" {
        state.workflow.active_node_id = node_id.to_string();
    }
}

fn set_workflow_stage_status(state: &mut ChainState, node_id: &str, status: &str) {
    if let Some(n) = state.workflow.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.status = status.to_string();
    }
}

fn upsert_tree_worker(state: &mut ChainState, worker_id: &str, status: &str) {
    let node_id = format!("worker:{worker_id}");
    let parent = format!("run:{}", state.training_run_id);
    if let Some(n) = state.tree.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.status = status.into();
        return;
    }
    state.tree.nodes.push(TreeNode {
        node_id,
        parent_id: parent,
        kind: "worker".into(),
        ref_id: worker_id.into(),
        status: status.into(),
        children_count: 0,
        meta: json!({}),
    });
    bump_children(state, &format!("run:{}", state.training_run_id));
}

fn upsert_tree_episode(state: &mut ChainState, episode_id: &str, ev: &ObservabilityEvent, status: &str) {
    let node_id = format!("episode:{episode_id}");
    let parent = if let Some(wid) = &ev.worker_id {
        if !wid.is_empty() {
            upsert_tree_worker(state, wid, "ACTIVE");
            format!("worker:{wid}")
        } else {
            format!("run:{}", state.training_run_id)
        }
    } else {
        format!("run:{}", state.training_run_id)
    };
    if let Some(n) = state.tree.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.status = status.into();
        n.parent_id = parent;
        n.meta = json!({
            "attempt_id": ev.attempt_id,
            "correlation_id": ev.correlation_id,
            "step_index": ev.step_index,
        });
        return;
    }
    state.tree.nodes.push(TreeNode {
        node_id,
        parent_id: parent.clone(),
        kind: "episode".into(),
        ref_id: episode_id.into(),
        status: status.into(),
        children_count: 0,
        meta: json!({
            "attempt_id": ev.attempt_id,
            "correlation_id": ev.correlation_id,
        }),
    });
    bump_children(state, &parent);
}

fn upsert_tree_step(state: &mut ChainState, episode_id: &str, step: i32, ev: &ObservabilityEvent) {
    let node_id = format!("step:{episode_id}:{step}");
    let parent = format!("episode:{episode_id}");
    upsert_tree_episode(state, episode_id, ev, "ACTIVE");
    if let Some(n) = state.tree.nodes.iter_mut().find(|n| n.node_id == node_id) {
        n.status = if ev.event_type == "STEP_COMPLETE" {
            "DONE".into()
        } else {
            "ACTIVE".into()
        };
        return;
    }
    state.tree.nodes.push(TreeNode {
        node_id,
        parent_id: parent.clone(),
        kind: "step".into(),
        ref_id: format!("{step}"),
        status: if ev.event_type == "STEP_COMPLETE" {
            "DONE".into()
        } else {
            "ACTIVE".into()
        },
        children_count: 0,
        meta: json!({ "step_index": step }),
    });
    bump_children(state, &parent);
}

fn close_tree_steps_for_episode(state: &mut ChainState, episode_id: &str, status: &str) {
    let parent = format!("episode:{episode_id}");
    for n in state.tree.nodes.iter_mut().filter(|n| n.parent_id == parent) {
        if n.kind == "step" && n.status == "ACTIVE" {
            n.status = status.into();
        }
    }
}

fn bump_children(state: &mut ChainState, parent_id: &str) {
    let count = state
        .tree
        .nodes
        .iter()
        .filter(|c| c.parent_id == parent_id)
        .count() as i32;
    if let Some(n) = state.tree.nodes.iter_mut().find(|n| n.node_id == parent_id) {
        n.children_count = count;
    }
}

#[allow(dead_code)]
pub fn ensure_seed_run(engine: &mut MergeEngine, run_id: &str) {
    let _ = engine.get_or_create(run_id);
    if let Some(life) = engine.lifecycles.get_mut(run_id) {
        if life.run_state == "PENDING" {
            life.run_state = "RUNNING".into();
        }
    }
    if let Some(state) = engine.runs.get_mut(run_id) {
        if state.run_state == "PENDING" {
            state.run_state = "RUNNING".into();
            state.updated_at = now_ms();
        }
    }
}
