// 文件职责：实现 episode admission 并发闸门，控制进入执行路径的 in-flight 数量。
// 主要功能：支持静态容量、动态容量、取消感知的 acquire，以及 worker 注册/注销时调整 permit。
// 大致工作流：submit 前先 acquire permit；episode 结束或取消后 permit 随 guard 释放，动态模式下容量跟随 worker 能力变化。

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::config::EpisodeConfig;

/// episode 进入执行流程前申请并发名额时可能出现的结果。
///
/// 这里的错误只表示“还没有进入 worker/agent 执行阶段”。调用方收到这些错误时，通常不需要
/// 释放 worker lease，因为还没有成功分配 worker。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionAcquireError {
    /// 客户端或管理接口取消了 episode。
    Cancelled,
    /// 等待队列名额直到 episode deadline 仍未成功。
    TimedOut,
    /// semaphore 被关闭。正常 server 生命周期中一般不会出现。
    Closed,
}

/// 控制进入 server 执行区的 episode 数量。
///
/// 支持三种模式：
/// 1. `queue_max_in_flight == 0` 且 `queue_dynamic == false`：不限制并发，`semaphore` 为 None。
/// 2. `queue_max_in_flight > 0`：使用固定容量 semaphore。
/// 3. `queue_dynamic == true`：容量随 worker 注册、心跳容量变化、下线而变化。
pub struct AdmissionController {
    /// 有并发限制时使用 semaphore；无限制模式下为 None。
    semaphore: Option<Arc<Semaphore>>,
    /// 是否采用动态容量。动态模式下 capacity delta 来自 scheduler/control plane。
    dynamic: bool,
}

impl AdmissionController {
    /// 根据配置创建 admission controller。
    ///
    /// 动态模式初始容量为 0，因为此时还不知道 worker 总容量；后续由 worker 注册和心跳更新。
    pub fn new(config: &EpisodeConfig) -> Self {
        let semaphore = if config.queue_dynamic {
            Some(Arc::new(Semaphore::new(0)))
        } else if config.queue_max_in_flight > 0 {
            Some(Arc::new(Semaphore::new(config.queue_max_in_flight)))
        } else {
            None
        };
        Self {
            semaphore,
            dynamic: config.queue_dynamic,
        }
    }

    pub fn is_dynamic(&self) -> bool {
        self.dynamic
    }

    /// 返回当前可用名额数。
    ///
    /// 返回 -1 表示无限制模式，调用方不应把它当作实际容量。
    pub fn available_permits(&self) -> i64 {
        self.semaphore
            .as_ref()
            .map(|s| s.available_permits() as i64)
            .unwrap_or(-1)
    }

    pub async fn acquire_until(
        &self,
        cancel_token: &CancellationToken,
        deadline: Instant,
    ) -> Result<Option<OwnedSemaphorePermit>, AdmissionAcquireError> {
        let Some(semaphore) = &self.semaphore else {
            // 无限制模式不需要持有 permit。返回 None 可以让调用方用同一套控制流处理。
            return Ok(None);
        };
        let deadline_sleep = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
        tokio::pin!(deadline_sleep);
        tokio::select! {
            _ = cancel_token.cancelled() => Err(AdmissionAcquireError::Cancelled),
            _ = &mut deadline_sleep => Err(AdmissionAcquireError::TimedOut),
            permit = semaphore.clone().acquire_owned() => {
                permit.map(Some).map_err(|_| AdmissionAcquireError::Closed)
            }
        }
    }

    pub fn on_capacity_changed(&self, old_capacity: u32, new_capacity: u32) {
        if !self.dynamic {
            return;
        }
        let Some(semaphore) = &self.semaphore else {
            return;
        };
        if new_capacity > old_capacity {
            semaphore.add_permits((new_capacity - old_capacity) as usize);
        } else if old_capacity > new_capacity {
            // 减少容量时不能直接删除正在被持有的 permit。这里在后台等待足够数量的 permit
            // 归还后再 forget，从而保证已经开始的 episode 不会被中途抢占。
            Self::shrink_async(Arc::clone(semaphore), old_capacity - new_capacity);
        }
    }

    pub fn on_worker_removed(&self, old_capacity: u32) {
        self.on_capacity_changed(old_capacity, 0);
    }

    fn shrink_async(semaphore: Arc<Semaphore>, reduce: u32) {
        if reduce == 0 {
            return;
        }
        tokio::spawn(async move {
            if let Ok(permit) = semaphore.acquire_many(reduce).await {
                permit.forget();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn static_admission_limits_concurrency() {
        let mut cfg = EpisodeConfig::default();
        cfg.queue_max_in_flight = 1;
        let admission = AdmissionController::new(&cfg);
        let token = CancellationToken::new();
        let permit = admission
            .acquire_until(&token, Instant::now() + std::time::Duration::from_secs(1))
            .await
            .expect("acquire")
            .expect("limited mode permit");
        assert_eq!(admission.available_permits(), 0);
        drop(permit);
        assert_eq!(admission.available_permits(), 1);
    }

    #[tokio::test]
    async fn dynamic_capacity_changes_adjust_permits() {
        let mut cfg = EpisodeConfig::default();
        cfg.queue_dynamic = true;
        let admission = AdmissionController::new(&cfg);
        assert_eq!(admission.available_permits(), 0);
        admission.on_capacity_changed(0, 2);
        assert_eq!(admission.available_permits(), 2);
        admission.on_worker_removed(1);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert_eq!(admission.available_permits(), 1);
    }
}
