// 文件职责：作为 Agent 池模块入口，把拆分后的类型、注册表实现和测试组合成原来的 agent_pool 模块。
// 主要功能：保持 crate::agent_pool::* 的外部 API 不变，同时把大文件物理拆分为 types、registry、tests。
// 大致工作流：编译时按 include! 顺序拼入分片文件，因此各分片仍共享同一个模块作用域。

// agent_pool module is split into focused source chunks while preserving the original module API.
include!("types.rs");
include!("registry.rs");
include!("tests.rs");
