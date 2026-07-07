//! uenv-common：worker 与 server 共享的契约类型。
//!
//! 只放"过 HTTP/JSON 的小契约"：轨迹引用、上传状态、服务端索引头。
//! 重型 `TrajectoryBundle`（含 steps + artifact）仍留在 worker；server 把
//! 上传的 bundle 正文当 opaque blob 存盘，仅用 [`TrajectoryHeader`] 抠出索引字段。

pub mod trajectory;

pub use trajectory::{TrajectoryHeader, TrajectoryRef, UploadStatus};
