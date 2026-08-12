# SWE-smith 长测试列表命令过长修复报告

> 日期：2026-08-11
> 关联说明：`Docs/adapter/20260811-SWE-smith长测试列表导致Worker命令过长说明.md`
> 关联样本：`pyparsing__pyparsing.533adf47.combine_file__dsi7jva0`

## 1. 结论

问题描述属实。该样本的 `FAIL_TO_PASS` 有 476 条、`PASS_TO_PASS` 有 1315 条，Worker 原实现会把全部 pytest node id 拼入一条 shell command，再作为：

```text
docker exec <container> bash -lc <完整命令>
```

的最后一个 argv 传给 `docker`。命令超过系统 `execve` 参数限制后，在容器内测试尚未启动前就失败：

```text
docker exec spawn failed: Argument list too long (os error 7)
```

## 2. 根因

原调用链位于 `uenv-worker/src/swe/session.rs`：

```rust
let test_cmd = self.instance.resolved_test_command(TESTBED);
let test_run = self.exec_raw(&test_cmd)?;
```

`exec_raw()` 使用：

```rust
Command::new(self.runtime.cli())
    .args(["exec", &self.container, "bash", "-lc", command])
```

而通用测试命令构造在 `repo_specs.rs` 中直接连接所有 node id：

```text
python -m pytest ... '<nodeid-1>' '<nodeid-2>' ... '<nodeid-1791>'
```

这使超长数据同时进入宿主机 `docker` 进程的 argv，而不是仅仅在容器内作为标准输入或文件内容处理。因此该问题不是 pytest 断言失败，也不是 gRPC 消息上限问题，而是 Worker 启动 `docker exec` 时的宿主机参数长度限制。

## 3. 修复方式

### 3.1 长列表写入容器文件

当 node id 总长度超过 `32 KiB` 时，`SweSession::evaluate()` 不再将列表内联到命令：

1. 将所有 `FAIL_TO_PASS` 和 `PASS_TO_PASS` node id 写入容器内：

   ```text
   /tmp/uenv-pytest-nodeids
   ```

2. 文件内容使用 NUL 分隔，避免 node id 中的空白字符被错误拆分。
3. 使用 `write_file()` 的临时文件 + `docker cp` 路径传输，node id 内容不会进入 `docker exec` argv。

### 3.2 容器内分批执行

长列表的 pytest 命令改为：

```bash
xargs -r -0 -n 100 python -m pytest -rA -v -p no:cacheprovider < /tmp/uenv-pytest-nodeids
```

这会在容器内每批最多执行 100 个 node id。每批 stdout/stderr 仍合并到同一份 Worker 测试结果中，现有 pytest parser 和 reward 决策逻辑无需改变。

### 3.3 适用范围

- 短测试列表保持原有 inline 命令，不改变正常样本行为。
- 通用 pytest、已登记 Django runner、SymPy `bin/test` runner 均支持文件分批路径。
- 显式 `test_cmd` 和 Pro 专用命令不自动改写，避免破坏其自定义命令语义。
- 长列表阈值为 `32 * 1024` 字节，低于 Linux 常见 `ARG_MAX`，同时为 shell、环境变量和 Docker 自身参数保留余量。

## 4. 修改文件

```text
uenv-worker/src/swe/session.rs
uenv-worker/src/swe/repo_specs.rs
uenv-worker/src/swe/harness.rs
```

新增的共享常量：

```rust
PYTEST_NODE_IDS_FILE = "/tmp/uenv-pytest-nodeids"
MAX_INLINE_NODE_IDS_BYTES = 32 * 1024
```

并增加了长 node list 命令构造测试，验证长命令包含 `xargs` 和容器文件路径，且不再包含原始 node id。

## 5. 验证

### 5.1 本地验证

通过：

```text
cargo fmt --all
python3 -m unittest tests.test_swe_catalog_tool
5 tests passed
```

本机 `cargo test` 仍因开发环境缺少 `protoc` 无法启动构建：

```text
Could not find `protoc`
```

### 5.2 7143 Worker 验证

已将三个修改后的 Rust 源文件同步到 7143，并在服务器上完成 release build：

```text
cargo build -p uenv-worker --release
Finished `release` profile [optimized]
```

随后重启 Worker，服务状态正常：

```text
28777/health -> ok
28097/runtime/v1/health -> ok
28888 -> LISTEN
28777 -> LISTEN
28097 -> LISTEN
```

Worker 日志确认重新注册并加载两个 SWE package：

```text
worker_start
register
swe-bench-pro images=731
swe-bench-smith images=59136
heartbeat
```

### 5.3 样本级验证边界

本次已确认问题根因、完成命令路径修复、完成 7143 release build 和 Worker 重启。但没有直接重新运行该样本完整的 1791 条测试，因为这会启动长时间测试任务并消耗训练侧资源。

代码级测试保证长列表不再进入 `docker exec` 的 command argv；7143 release build 保证部署二进制包含该逻辑。后续执行该样本时应重点确认日志不再出现：

```text
docker exec spawn failed: Argument list too long
```

并关注分批 pytest 的总输出是否完整覆盖全部 node id。

## 6. 后续注意事项

`xargs` 的批量执行会让相同测试 fixture 在不同 pytest 进程中重复初始化，这是为避免宿主机 argv 限制做出的稳定性优先取舍。若未来需要降低总耗时，可进一步实现容器内 Python runner，一次读取 node id 文件并调用 pytest API；这不影响本次修复的正确性。
