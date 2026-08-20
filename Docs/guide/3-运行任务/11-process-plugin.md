# 自定义环境

UEnv 内置的 `qa`、`code` 和 SWE 环境覆盖不了的任务，可以用 process plugin 自建环境：一个运行在 UEnv Worker 上的 Python 插件，由你自己定义任务初始化、交互步骤和判分方式。本页用安装包自带的可运行模板走一遍完整过程——创建、改造、安装、评测、训练、分发到多台 Worker；模板环境叫 `my-environment`，逻辑是让模型只回复 `ok`，照此替换为自己的任务逻辑即可接入真实环境。

| 项目 | 本案例取值 |
|---|---|
| 环境 / 路由 | `my-environment` / `my-dataset` |
| 环境契约 | `expected_action=ok`，plugin reward target 为 `ok` |
| 评测输入 | 创建时生成的 `example.jsonl` |
| 训练输入 | `examples/cases/training/process-plugin.jsonl` |
| 当前训练实现 | VeRL v0.7.1、GRPO |

## 什么时候需要新建环境

| 新任务的需求 | 扩展方式 |
|---|---|
| 交互和判分方式与已有环境相同，只是数据不同 | 在已有环境中增加 `dataset` 和判分实现，不用新建 plugin |
| 任务是生成代码并运行测试 | 在 `code` 环境中增加 `dataset` 和测试实现 |
| 任务初始化、交互格式或判分方式与已有环境不同 | 创建新的 process plugin（本页流程） |
| 任务需要容器、外部执行程序或较长执行过程 | 实现任务专用的运行组件；SWE 的实现可作代码参考 |

## 流程总览

1. 在一台 UEnv Worker 主机创建模板。
2. 按[要改哪些文件](#改造成真实环境要改哪些文件)把自己的任务逻辑写进模板；只想先验证工具链时可以不改，模板能原样跑通。
3. 测试并安装到当前 Worker（`install-local` 只影响本机）。
4. 用评测入口 `uenv evaluate run-task` 做最小验证（UEnv 侧无需 GPU，模型可来自云端 API 或别处已部署的模型服务）。
5. 需要训练时，在 GPU 主机执行 `uenv train run-task`。
6. 多台 UEnv Worker 或需要固定版本时，通过 UEnv Hub 发布并同步到每台 Worker。

## 创建模板

在 UEnv Worker 主机执行。安装包提供可执行工具和一个确定性单步模板：

```bash
export UENV_RELEASE_ROOT='/opt/uenv/current'
export PLUGIN_TOOL="$UENV_RELEASE_ROOT/libexec/uenv/environment/plugin.sh"
export PLUGIN_DIR="$PWD/uenv-envs/my-environment"

test -x "$PLUGIN_TOOL"
test ! -e "$PLUGIN_DIR"
mkdir -p "$(dirname "$PLUGIN_DIR")"

"$PLUGIN_TOOL" create my-environment \
  --dataset my-dataset \
  --dir "$PLUGIN_DIR"
```

刚创建的模板已经实现本案例需要的 `expected_action=ok` 单步逻辑，可以不做任何修改直接跑通本页流程。

## 改造成真实环境：要改哪些文件

接入真实任务时，只改下表中标记“必须/按需”的文件；不要把模板 reward 当成业务环境。

| 文件 | 是否修改 | 改什么 |
|---|---|---|
| `environment.py` | 必须 | `SELF_TEST_CASE`（自测用例）、`reset()`（任务初始化）、`step()`（模型动作处理与环境返回）、`reward()`（得分计算） |
| `example.jsonl` | 必须 | 至少一条可运行的示例输入，字段与环境的 `env_config` / `reward_config` 契约一致 |
| `manifest.yaml` | 必须 | `env_type`、`datasets`；行为变化（判分、依赖、接口）先升 `version` 再发布 |
| `requirements.txt` | 按需 | plugin 使用的第三方 Python 依赖 |
| `plugin.py`、`run.sh`、`uenv_plugin_api.py`、`generated/`、`tests/` | 不要改 | UEnv 通信与协议文件，保持生成时的内容 |

## 测试并安装到当前 UEnv Worker

沿用前面的 `PLUGIN_TOOL`、`PLUGIN_DIR` 变量。每次修改文件后都重新执行测试和安装；安装是原子的，并会重启 UEnv Worker：

```bash
"$PLUGIN_TOOL" test "$PLUGIN_DIR"
sudo "$PLUGIN_TOOL" install-local "$PLUGIN_DIR"
sudo systemctl is-active uenv-worker.service
```

在 UEnv Server 主机确认 UEnv Worker 恢复 ready 并上报了新环境类型：

```bash
uenv workers
curl -fsS http://127.0.0.1:50052/status | \
  jq -r '.workers[] | [.endpoint, .status, (.supported_env_types | join(","))] | @tsv'
```

第二条命令每台 Worker 输出一行，第三列是它支持的环境类型。至少一台 `ready` Worker 的第三列应包含 `my-environment`。

## 用评测验证环境

环境装好后，先用评测入口做最小验证：UEnv 链路只通过网络调用模型 API，UEnv Server、UEnv Worker 和客户端主机都不需要 GPU。模型来自云端 API（如火山引擎方舟）时全程不需要 GPU；如果模型需要本地部署（vLLM、SGLang 等），先在一台有 GPU 的主机上启动模型服务，UEnv Worker 通过网络访问它。

**前置条件**：`run-task` 由 UEnv Worker 调用模型，首次运行前必须在每台可能接单的 UEnv Worker 上执行过一次 `uenv evaluate configure-model`（本地与云端两种配置方式见[通用评测流程](./03-evaluation.md#配置并检查模型-api)）；未配置的 Worker 接单后 Episode 会因模型调用失败。

本验证在任意能访问 UEnv Server 50051/TCP 的客户端主机执行。plugin 目录下的 `example.jsonl` 是创建时生成的示例输入（内容就是要求模型回复 `ok`）；在客户端主机执行时先把它复制到本机。

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export INPUT="$PWD/example.jsonl"
export RUN_ID="plugin-eval-$(date +%Y%m%d-%H%M%S)"
export OUTPUT="$PWD/results/$RUN_ID/results.jsonl"

test -r "$INPUT"
jq -e -c . "$INPUT" >/dev/null
mkdir -p "$(dirname "$OUTPUT")"

uenv evaluate run-task \
  --endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type my-environment \
  --dataset my-dataset \
  --input "$INPUT" \
  --output "$OUTPUT" \
  --max-steps 1 \
  --batch-size 1 \
  --streaming
```

多机部署把 `UENV_SERVER_ENDPOINT` 换成 UEnv Server 实际地址。验收终态与数值 reward：

```bash
jq -e -s '
  length == 1 and
  .[0].status == "completed" and
  (.[0].reward | type) == "number"
' "$OUTPUT" >/dev/null && echo 'custom environment evaluation completed'
```

模型按提示回复 `ok` 时 reward 为 1，否则为 0；`status=completed` 即表示环境链路完整。

## 用于强化学习训练

训练入口在有 NVIDIA GPU、模型和容器运行时的训练主机执行。先设置实际模型、镜像和唯一工作目录并检查：

```bash
export UENV_SERVER_ENDPOINT='127.0.0.1:50051'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export MODEL_DIR='/absolute/path/to/huggingface-model'
export TRAIN_IMAGE='docker.io/verlai/verl:vllm017.latest'
export INPUT="$UENV_RELEASE_ROOT/examples/cases/training/process-plugin.jsonl"
export TRAIN_CONFIG="$UENV_RELEASE_ROOT/examples/cases/training/verl-grpo-overrides.conf"
export RUN_ID="custom-env-train-$(date +%Y%m%d-%H%M%S)"
export WORK_DIR="$PWD/uenv-runs/$RUN_ID"

nvidia-smi -L
docker info >/dev/null
test -d "$MODEL_DIR"
test -r "$INPUT"
test -r "$TRAIN_CONFIG"
jq -e -c . "$INPUT" >/dev/null
test ! -e "$WORK_DIR"
mkdir -p "$(dirname "$WORK_DIR")"
docker image inspect "$TRAIN_IMAGE" >/dev/null || docker pull "$TRAIN_IMAGE"
```

源码运行时替换 release root；多机时替换 UEnv Server 地址并配置 UEnv Worker 可达的 model gateway。确认变量后执行训练：

```bash
uenv train run-task \
  --model "$MODEL_DIR" \
  --work-dir "$WORK_DIR" \
  --uenv-endpoint "$UENV_SERVER_ENDPOINT" \
  --env-type my-environment \
  --dataset my-dataset \
  --input "$INPUT" \
  --max-steps 1 \
  --gpus 1 \
  --steps 50 \
  --rollouts 4 \
  --train-batch-size 2 \
  --runtime docker \
  --image "$TRAIN_IMAGE" \
  --verl-config "$TRAIN_CONFIG"
```

当前公共训练入口只接受 `max_steps=1`。需要多步环境时，环境本身要正确实现中间 `terminated=false`，强化学习接入实现还必须提供完整多步 response trace；不能只提高命令参数。

plugin 应收到 `expected_action=ok` 与 reward target，并返回确定终态；当前 runner 应完成 50 次更新并产生数据与训练输出。验收：

```bash
test -d "$WORK_DIR/episode-data"
test -d "$WORK_DIR/.uenv-verl/output"
find "$WORK_DIR/episode-data" -type f -print -quit | grep -q .
find "$WORK_DIR/.uenv-verl/output" -mindepth 1 -print -quit | grep -q .
echo 'custom environment training artifacts present'
```

还要从 UEnv Worker journal 验证 plugin 初始化、动作、判分与每个 Episode 的资源回收：

```bash
sudo journalctl -u uenv-worker.service -n 200 --no-pager
```

## 分发到多台 UEnv Worker（UEnv Hub）

- `install-local` 只把 plugin 装到**当前这台** Worker 并重启生效；Worker 重启后加载它，并向 UEnv Server 上报新的环境类型。
- 只有一台 Worker 的试验环境用 `install-local` 就够了。**多台 Worker 或需要固定/回滚版本**时，走 UEnv Hub：发布主机把 plugin 发布为 EnvPackage，每台 Worker 预同步、激活并重启。
- **UEnv Hub 不在任务运行链路上**：Episode 的调度和执行只发生在 UEnv Server 与 UEnv Worker 之间；Hub 只负责环境版本的登记、保存和分发。Hub 本身的部署和令牌创建见[部署和使用 UEnv Hub](../2-部署UEnv/05-hub.md)。

前提：UEnv Hub 已部署，发布主机持有发布者（publisher）令牌，每台 Worker 已配置只读（reader）令牌（见[让 UEnv Worker 连接 UEnv Hub](../2-部署UEnv/05-hub.md#让-uenv-worker-连接-uenv-hub)）。

第一步，在发布主机（plugin 目录所在主机）登录并发布：

```bash
export HUB_ENDPOINT='http://10.0.0.15:8080'
export PLUGIN_DIR="$PWD/uenv-envs/my-environment"

uenv hub login \
  --endpoint "$HUB_ENDPOINT" \
  --token-file "$(realpath environment-publisher.token)"

uenv env plugin publish "$PLUGIN_DIR"
```

发布前确认 `manifest.yaml` 里的 `version` 已按本次修改递增；同名同版本发布后内容保持不变。第二步，在每台 UEnv Worker 上同步、激活并重启（`--worker-version` 填本机 `uenv version` 显示的版本号）：

```bash
sudo uenv env sync my-environment \
  --version 0.1.1 \
  --target-dir /var/lib/uenv \
  --consumer worker \
  --worker-version '0.1.0+57aad73' \
  --activate \
  --plugin-dir /var/lib/uenv/plugins

sudo systemctl restart uenv-worker.service
uenv environments
```

`uenv environments` 应列出 `my-environment` 且版本为刚激活的版本；之后在 UEnv Server 主机按[前文](#测试并安装到当前-uenv-worker)的 `/status` 命令核对每台 Worker 都上报了 `my-environment`。多台 Worker 逐台重复第二步。

## 失败定位

| 现象 | 处理 |
|---|---|
| 没有可用 UEnv Worker | 核对 plugin 是否安装、激活并正确上报 capability |
| plugin schema 错误 | 对照环境自己的 schema 修正输入，不在接入代码里猜字段 |
| reward 为空 | 检查 plugin stderr、协议输出和终态判分 |
| 进程持续增加 | 修复 Episode close/timeout 资源清理后再训练 |
| 多步 trace 缺失 | 同时修环境生命周期与强化学习接入的 token trace |
