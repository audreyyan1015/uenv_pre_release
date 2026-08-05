# UEnv 自定义环境模板

这个模板把“环境逻辑”和“UEnv 通信协议”分开了。使用 `plugin.sh create`
创建目录后，第一次只需要修改 `environment.py` 中的自测样例和
`reset()`、`step()`、`reward()`。换示例输入时改 `example.jsonl`；发布新行为时递增
`manifest.yaml` 的 `version`；需要其他 Python 库时才在 `requirements.txt` 末尾追加。

不要修改 `plugin.py`、`run.sh`、`uenv_plugin_api.py` 和 `generated/`。它们负责
Worker 与环境之间的 gRPC/Unix socket 通信，不包含任务规则。

`environment.py` 顶部的 `SELF_TEST_CASE` 也是环境逻辑的一部分。替换示例任务时
同步更新这个小样例，固定的测试程序会自动读取它，无需再修改 `tests/`。

## 最短使用路径

在源码仓库中：

```bash
./examples/environment/plugin.sh create my-environment --dataset my-dataset
# 编辑 my-environment/environment.py
./examples/environment/plugin.sh test my-environment
sudo ./examples/environment/plugin.sh install-local my-environment
```

评测时明确写出环境、数据集和输入文件；与内置 QA/Code 的命令结构相同：

```bash
uenv evaluate run-task \
  --endpoint 127.0.0.1:50051 \
  --env-type my-environment \
  --dataset my-dataset \
  --input ./my-environment/example.jsonl \
  --output ./my-environment/results.jsonl \
  --max-steps 1
```

使用安装包时，入口位于：

```bash
/opt/uenv/current/examples/environment/plugin.sh
```

本地安装会把代码放进不可变的版本目录，原子切换
`/var/lib/uenv/plugins/<env_type>`，然后重启并检查 `uenv-worker`。因此不需要手工
复制插件、创建 venv 或修改 Worker 配置。

要发布到已经登录的 Hub：

```bash
./examples/environment/plugin.sh publish my-environment
```

该命令会自动运行测试、创建 `.venv`、准备 `wheelhouse/`，再调用
`uenv env publish-plugin`。wheel 必须在与目标 Worker 相同的 Linux 架构和 Python
版本上生成。内网机器可先在匹配平台的联网机器执行一次 `publish` 来准备
`wheelhouse/`，之后使用 `test --offline`、`install-local --offline` 或
`publish --offline`，这些命令不会访问 Python 包索引。

## `environment.py` 接口

每个插件进程对应一个环境实例：

- `reset(config, seed) -> ResetResult` 初始化 Episode；
- `step(action: bytes) -> StepResult` 执行一步；
- `reward(action: str) -> float` 放奖励逻辑；
- `close()` 释放环境持有的资源。

`config` 有两种兼容视图：常用 `env_config` 字段会出现在顶层；完整的原始请求位于
`config["_uenv"]`，其中包含 `payload`、`reward_config`、`seed` 和 sidecar
schema 版本。无需修改 Bridge、Worker 或 protobuf 来增加任务字段。

观察值可以是 `str`、`bytes`、字典或列表；字典和列表会编码为 UTF-8 JSON。
`info` 的值会统一转换成协议要求的字符串。一个 Episode 想运行多步时，只需让
`step()` 在中间步骤返回 `terminated=False`，在结束时返回 `terminated=True`。

先运行不需要第三方依赖的逻辑测试：

```bash
./examples/environment/plugin.sh test my-environment --logic-only
```

完整测试会启动真实 Unix socket，依次调用 `HealthCheck`、`Reset`、`Step` 和
`Close`：

```bash
./examples/environment/plugin.sh test my-environment
```

## 升级协议

生成的 Python stub 已随模板提供。只有当 UEnv 的共享 `plugin.proto` 升级时，才在
源码仓库里安装 `requirements-dev.txt` 并运行 `generate_proto.sh`；普通环境开发不
需要执行这一步。
