// 文件职责：作为 trajectory 模块入口，把配置、存储、HTTP 和测试分片组合成原来的 trajectory 模块。
// 主要功能：保持 crate::trajectory::* 外部 API 不变，同时把大文件按 config/store/http/tests 物理拆分。
// 大致工作流：编译时按 include! 顺序拼入分片文件，各分片仍共享同一个 trajectory 模块作用域。

// trajectory module is split by configuration, storage, HTTP, and tests.
include!("prelude.rs");
include!("config.rs");
include!("store.rs");
include!("http.rs");
include!("tests.rs");
