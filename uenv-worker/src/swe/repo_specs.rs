//! repo_specs — 官方 `MAP_REPO_VERSION_TO_SPECS` 子集（plan §1.2 / gap M1-2 / M1-4）。
//!
//! SWE-bench 官方 harness 为每个 `repo@version` 维护 install / test 命令与日志解析器
//! （`princeton-nlp/SWE-bench` 的 `constants.py`）。Worker 离线运行时，无法 `import` 该
//! 常量表，故在此**移植一个可扩展的子集**：覆盖 Verified/Lite 高频仓库，并对未登记仓库
//! 回退到通用 pytest（保持既有行为）。
//!
//! 每条规格给出：
//! - [`TestRunner`]：如何构造测试命令（pytest 节点 id / Django test label / 自定义模板）；
//! - `install`：可选 post-patch 安装命令（`pip install -e .` 等，M1-3 实际生效来源之一）；
//! - [`LogParser`]：测试输出口径（pytest `PASSED` / Django `... ok`），供 grader 选择。
//!
//! 优先级（`SweInstance::resolved_*`）：实例显式字段 > 本表 `repo@version` > 通用 pytest 兜底。

/// 测试输出日志解析器口径（grader 据此选择解析函数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogParser {
    /// pytest：`<nodeid> PASSED` / `PASSED <nodeid>`（含 `::`）。
    Pytest,
    /// Django 自带 runner：`test_x (mod.Cls) ... ok|FAIL|ERROR`。
    Django,
}

impl Default for LogParser {
    fn default() -> Self {
        Self::Pytest
    }
}

/// 测试 runner（如何把 FAIL/PASS 节点 id 拼成容器内可执行的测试命令）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestRunner {
    /// `python -m pytest <flags> <ids>`。
    Pytest,
    /// Django：`./tests/runtests.py --settings=test_sqlite --verbosity 2 --parallel 1 <labels>`。
    Django,
    /// sympy 旧版：`bin/test -C --verbose <ids>`。
    SympyBinTest,
}

/// 单个 `repo@version`（或 repo 全版本）的执行规格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoSpec {
    pub runner: TestRunner,
    pub log_parser: LogParser,
    /// post-patch 安装命令（None = 依赖镜像已 setup，不再安装）。
    pub install: Option<&'static str>,
    /// pytest flags（仅 `TestRunner::Pytest` 使用）。
    pub pytest_flags: &'static str,
}

impl RepoSpec {
    const fn pytest(install: Option<&'static str>) -> Self {
        Self {
            runner: TestRunner::Pytest,
            log_parser: LogParser::Pytest,
            install,
            pytest_flags: "-rA -v -p no:cacheprovider",
        }
    }

    const fn django() -> Self {
        Self {
            runner: TestRunner::Django,
            log_parser: LogParser::Django,
            install: None,
            pytest_flags: "",
        }
    }

    /// 构造容器内完整测试命令（含 conda activate + cd testbed + runner + 节点 id）。
    pub fn build_test_command(
        &self,
        conda_activate: &str,
        testbed: &str,
        fail_to_pass: &[String],
        pass_to_pass: &[String],
    ) -> String {
        let ids = fail_to_pass
            .iter()
            .chain(pass_to_pass.iter())
            .map(|id| single_quote(id))
            .collect::<Vec<_>>()
            .join(" ");
        match self.runner {
            TestRunner::Pytest => format!(
                "{conda_activate}; cd {testbed} && python -m pytest {} {ids}",
                self.pytest_flags
            ),
            TestRunner::Django => format!(
                "{conda_activate}; cd {testbed} && ./tests/runtests.py --verbosity 2 --settings=test_sqlite --parallel 1 {ids}"
            ),
            TestRunner::SympyBinTest => format!(
                "{conda_activate}; cd {testbed} && bin/test -C --verbose {ids}"
            ),
        }
    }
}

/// 通用兜底规格（未登记仓库）：与历史行为一致的 pytest。
pub const DEFAULT_SPEC: RepoSpec = RepoSpec::pytest(None);

/// 查询 `repo@version` 的执行规格。
///
/// `repo` 形如 `django/django`；`version` 可为空（按 repo 默认）。命中返回登记规格，
/// 未命中返回 `None`（调用方回退 [`DEFAULT_SPEC`]）。表为可扩展子集，新增仓库在此登记即可。
pub fn spec_for(repo: &str, version: &str) -> Option<RepoSpec> {
    let repo = repo.trim().to_ascii_lowercase();
    let spec = match repo.as_str() {
        // Django 自带 runner（非 pytest 节点 id，输出口径不同）。
        "django/django" => RepoSpec::django(),

        // 需 editable install 的仓库（镜像未必预装当前源码）。
        "scikit-learn/scikit-learn" => RepoSpec::pytest(Some("pip install -e . --no-build-isolation")),
        "matplotlib/matplotlib" => RepoSpec::pytest(Some("pip install -e .")),
        "pydata/xarray" => RepoSpec::pytest(Some("pip install -e .")),

        // sympy/sympy：1.9 前用 `bin/test`（节点 id 为短 label）；1.9+ 用 pytest。
        "sympy/sympy" => {
            if sympy_uses_bin_test(version) {
                RepoSpec {
                    runner: TestRunner::SympyBinTest,
                    log_parser: LogParser::Pytest,
                    install: None,
                    pytest_flags: "",
                }
            } else {
                RepoSpec::pytest(None)
            }
        }

        // 纯 pytest（镜像已 setup，无需再装）。
        "astropy/astropy"
        | "pytest-dev/pytest"
        | "sphinx-doc/sphinx"
        | "pylint-dev/pylint"
        | "pallets/flask"
        | "psf/requests"
        | "mwaskom/seaborn"
        | "pydicom/pydicom"
        | "psf/black"
        | "marshmallow-code/marshmallow"
        | "pyvista/pyvista"
        | "sqlfluff/sqlfluff" => RepoSpec::pytest(None),

        _ => return None,
    };
    Some(spec)
}

/// sympy 旧版（< 1.9）使用 `bin/test`，节点 id 为短 label 而非 pytest nodeid。
fn sympy_uses_bin_test(version: &str) -> bool {
    parse_version_major_minor(version)
        .map(|(maj, min)| maj < 1 || (maj == 1 && min < 9))
        .unwrap_or(true)
}

fn parse_version_major_minor(version: &str) -> Option<(u32, u32)> {
    let head = version.split(|c| c == '.' || c == '-' || c == '+').take(2);
    let mut parts = head.filter_map(|p| p.parse::<u32>().ok());
    Some((parts.next()?, parts.next().unwrap_or(0)))
}

fn single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sympy_version_selects_bin_test_for_old_releases() {
        assert_eq!(spec_for("sympy/sympy", "1.8").unwrap().runner, TestRunner::SympyBinTest);
        assert_eq!(spec_for("sympy/sympy", "1.12").unwrap().runner, TestRunner::Pytest);
    }

    #[test]
    fn known_repo_resolves_runner() {
        assert_eq!(spec_for("django/django", "4.1").unwrap().runner, TestRunner::Django);
        assert_eq!(spec_for("Django/Django", "").unwrap().log_parser, LogParser::Django);
        assert_eq!(spec_for("astropy/astropy", "5.0").unwrap().runner, TestRunner::Pytest);
        assert_eq!(
            spec_for("scikit-learn/scikit-learn", "1.3").unwrap().install,
            Some("pip install -e . --no-build-isolation")
        );
    }

    #[test]
    fn unknown_repo_is_none_and_falls_back() {
        assert!(spec_for("acme/unknown", "1.0").is_none());
        assert_eq!(DEFAULT_SPEC.runner, TestRunner::Pytest);
        assert_eq!(DEFAULT_SPEC.log_parser, LogParser::Pytest);
        assert!(DEFAULT_SPEC.install.is_none());
    }

    #[test]
    fn pytest_command_quotes_ids_and_uses_flags() {
        let spec = DEFAULT_SPEC;
        let cmd = spec.build_test_command(
            "source activate",
            "/testbed",
            &["pkg/test_x.py::test_a".to_string()],
            &["pkg/test_x.py::test_b[1-2]".to_string()],
        );
        assert!(cmd.contains("python -m pytest -rA -v -p no:cacheprovider"));
        assert!(cmd.contains("'pkg/test_x.py::test_a'"));
        assert!(cmd.contains("'pkg/test_x.py::test_b[1-2]'"));
        assert!(cmd.contains("cd /testbed"));
    }

    #[test]
    fn django_command_uses_runtests() {
        let spec = spec_for("django/django", "4.2").unwrap();
        let cmd = spec.build_test_command(
            "source activate",
            "/testbed",
            &["auth_tests.test_x.MyTest.test_a".to_string()],
            &[],
        );
        assert!(cmd.contains("./tests/runtests.py"));
        assert!(cmd.contains("--settings=test_sqlite"));
        assert!(cmd.contains("'auth_tests.test_x.MyTest.test_a'"));
    }

    #[test]
    fn sympy_bin_test_runner_builds_bin_test() {
        let spec = RepoSpec {
            runner: TestRunner::SympyBinTest,
            ..DEFAULT_SPEC
        };
        let cmd = spec.build_test_command("source activate", "/testbed", &["sympy/x.py::test_a".to_string()], &[]);
        assert!(cmd.contains("bin/test -C --verbose"));
    }
}
