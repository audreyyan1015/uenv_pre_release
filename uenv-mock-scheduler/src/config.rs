//! Mock Scheduler 配置占位（M1 实现 YAML 加载）

#[derive(Debug, Default)]
pub struct MockSchedulerConfig {
    pub listen: String,
    pub fixture_dir: String,
    pub log_file: String,
}
