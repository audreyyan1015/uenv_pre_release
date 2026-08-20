# 代码修复

## 任务与数据性质

本案例让实际模型在两个 Verified catalog 实例中修改仓库，并由 UEnv Worker Runtime Gateway 中的测试环境判分。输入 JSONL 只选择实例；问题、仓库、base commit、测试和镜像来自安装包固定 catalog。

| 项目 | 本案例取值 |
|---|---|
| variant | `verified` |
| 输入真源 | `examples/cases/evaluation/swe-verified.jsonl` |
| catalog | `share/swe/verified.json` |
| 实例 | `astropy__astropy-7166`、`psf__requests-1142` |
| 当前 Agent 运行细节 | 发布包固定的 OpenHands 实现 |

## 执行主机

在已启用 SWE Runtime 的 UEnv Worker 主机执行（启用方法见下文[启用 SWE Runtime](#启用-swe-runtime)）。`run-swe` 需要 `sudo` 读取 Gateway 凭据并使用容器运行时；模型 API 必须从该 UEnv Worker 可达。

## 启用 SWE Runtime

启用 SWE Runtime 是一次性运维操作，由 `prepare-swe` 完成：写入 SWE 配置（`/etc/uenv/swe.env`）、安装 release 固定版本的 OpenHands Agent、启动本机 Runtime Gateway（默认 `127.0.0.1:28999`）。

多机部署先在 UEnv Server 主机准备 Gateway 共享密钥：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile control-plane \
  --shared-key-file /home/uenv-install/uenv-swe-shared.key
```

`--shared-key-file` 指向的文件不存在时，会自动生成一个权限为 `0600` 的密钥；已存在则复用。把同一个密钥文件安全复制到每台 SWE UEnv Worker，然后在每台 Worker 上执行（替换为实际地址）：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile worker \
  --runtime docker \
  --image-policy local_only \
  --gateway 127.0.0.1:28999 \
  --gateway-public http://127.0.0.1:28999 \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.21:50054 \
  --trajectory-endpoint http://10.0.0.10:8077 \
  --shared-key-file /home/uenv-install/uenv-swe-shared.key
```

单机部署（`single-node` 或 `full` profile）不需要共享密钥和地址参数，一条命令完成：

```bash
sudo uenv evaluate prepare-swe \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile single-node \
  --runtime docker \
  --image-policy local_only \
  --gateway 127.0.0.1:28999
```

使用 Podman 时把 `--runtime docker` 改为 `--runtime podman`。`--image-policy local_only` 表示只使用本机已有镜像；允许从公共镜像仓库拉取时用 `allow_public`。

在 UEnv Worker 主机验证启用结果：

```bash
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:28999/runtime/v1/health
sudo -u uenv docker info >/dev/null
```

启用后 UEnv Worker 上报的环境类型自动包含 `swe`（核对方法见[通用评测流程](./03-evaluation.md#检查共同前置条件)）。注意事项：

- 已存在 `/etc/uenv/swe.env` 且与本次参数不同时，`prepare-swe` 会中止并提示 `--force-swe-config`；先备份并确认差异，再带该参数重跑。
- `control-plane` 形式的 `prepare-swe` 只配置密钥，不安装 Worker 组件；但安装器在 `control-plane` profile 下会停用同机的 `uenv-worker.service`，如果该主机同时运行 UEnv Worker，准备完成后用 `sudo systemctl enable --now uenv-worker.service` 恢复。
- `--trajectory-endpoint` 指向 UEnv Server 的轨迹接口；多机使用时需按[轨迹采集指南](./12-trajectory.md#多机部署)把 Server 侧 `8077` 改为受控内网地址并分发 token。

## 前置检查

设置实际模型 API、模型名和本轮唯一目录：

```bash
export MODEL_API='http://10.0.0.30:8000/v1'
export MODEL_NAME='your-code-model'
export UENV_RELEASE_ROOT='/opt/uenv/current'
export SWE_CATALOG="$UENV_RELEASE_ROOT/share/swe/verified.json"
export SWE_INPUT="$UENV_RELEASE_ROOT/examples/cases/evaluation/swe-verified.jsonl"
export RUN_ID="software-repair-eval-$(date +%Y%m%d-%H%M%S)"
export OUTPUT="$PWD/results/$RUN_ID/results.jsonl"
export ARTIFACTS="/var/lib/uenv/evaluation-runs/$RUN_ID"
```

在 UEnv Worker 主机执行检查：

```bash
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:28999/runtime/v1/health
sudo -u uenv docker info >/dev/null
curl -fsS "$MODEL_API/models" >/dev/null
test -r "$SWE_CATALOG"
test -r "$SWE_INPUT"
jq -e '
  has("astropy__astropy-7166") and
  has("psf__requests-1142")
' "$SWE_CATALOG" >/dev/null
test ! -e "$OUTPUT"
sudo test ! -e "$ARTIFACTS"
mkdir -p "$(dirname "$OUTPUT")"
```

使用 Podman 时替换容器检查；模型服务未实现 `GET /models` 时使用其官方健康请求。多机环境不把 `MODEL_API` 写成仅对其他主机有效的地址。

镜像准备的时序：`prepare-swe` 只安装运行组件，不拉取实例镜像；首次 `run-swe` 时才按输入选中的实例准备镜像。`--image-policy local_only` 下镜像必须事先在本机（`docker images` 可见），缺失即失败、不会临时拉取；`allow_public` 才允许运行时从镜像仓库拉取。离线或内网环境提前 `docker pull`，或按[部署和使用 UEnv Hub](../2-部署UEnv/05-hub.md)用 `image_bundle.sh` 把镜像 tar 从 UEnv Hub 分发到 UEnv Worker。

从源码工作区运行时，先把 `REPO_ROOT` 设为仓库根目录的绝对路径，再使用 `SWE_INPUT="$REPO_ROOT/examples/cases/evaluation/swe-verified.jsonl"` 和 `SWE_CATALOG="$REPO_ROOT/config/swe/verified.json"`；源码与安装包的 catalog 目录布局不同。

## 输入与 catalog

发布包自带的 catalog 是冒烟样例，不是完整数据集：`verified.json` 含 10 条实例，`smith-sample-catalog.json` 含 5 条。四个 variant（`verified`、`lite`、`pro`、`smith`）的完整 catalog 都可以用官方数据集导出生成，例如：

```bash
uenv evaluate build-swe-catalog \
  --variant verified \
  --input /path/to/official-export.json \
  --output /var/lib/uenv/swe-catalogs/swe-bench-verified.json
```

`--input` 支持官方 JSON、JSONL 或 Parquet 导出。生成的 catalog 路径传给 `run-swe --catalog`；variant、catalog 和输入 JSONL 必须对应同一基准。

SWE 输入 JSONL 每行只选择 catalog 中的一个实例：

```json
{"id":"case-name","instance_id":"owner__repo-1234"}
```

要求 `id` 唯一、`instance_id` 存在于 catalog、catalog 的 variant 与命令一致，并且 UEnv Worker Runtime Gateway 看到的是同一份实例元数据。`run-swe` 在创建运行目录前校验这些条件；校验失败时修正数据，不要绕过检查。

实例镜像的解析方式按 variant 区分：Verified/Lite 不需要 `image_cache_key` 字段，镜像名由 `instance_id` 按官方命名规则推导（`swebench/sweb.eval.*`）；Pro 和 Smith 必须在 catalog 中显式提供 `image_cache_key`。

## 配置模型 API

SWE 的模型调用方不是 UEnv Worker 主进程，而是实例容器中的 OpenHands Agent，因此模型配置不经过 `configure-model`，在 `run-swe` 命令行上直接声明 provider。与普通 Episode 支持的模型来源相同（本地 OpenAI-compatible 服务或云端方舟 API），只是配置位置不同：普通 Episode 的模型配置持久化在 UEnv Worker 上、所有任务共用；SWE 的模型参数随每次 `run-swe` 传入、只作用于当次运行。

本地部署的模型服务用 `--provider local --base-url "$MODEL_API" --model "$MODEL_NAME"`，完整命令见下文[执行](#执行)。云端火山引擎方舟的完整命令如下（`--model` 填推理接入点 ID，密钥从权限为 `0600` 的单行文件读取，或省略 `--api-key-file` 按提示交互输入）：

```bash
sudo uenv evaluate run-swe \
  --provider volcengine \
  --model 'ep-xxxxxxxx' \
  --api-key-file ./ark-api-key.txt \
  --gateway 'http://127.0.0.1:28999' \
  --catalog "$SWE_CATALOG" \
  --benchmark-variant verified \
  --input "$SWE_INPUT" \
  --output "$OUTPUT" \
  --artifacts-dir "$ARTIFACTS" \
  --max-iterations 30 \
  --batch-size 2
```

不要把密钥写进输入文件或提交到仓库。

## 执行

```bash
sudo uenv evaluate run-swe \
  --provider local \
  --model "$MODEL_NAME" \
  --base-url "$MODEL_API" \
  --gateway 'http://127.0.0.1:28999' \
  --catalog "$SWE_CATALOG" \
  --benchmark-variant verified \
  --input "$SWE_INPUT" \
  --output "$OUTPUT" \
  --artifacts-dir "$ARTIFACTS" \
  --max-iterations 30 \
  --batch-size 2
```

模型 API 需要密钥时，使用权限为 `0600` 的单行文件并增加 `--api-key-file /secure/path/key`；不要把 key 直接写入命令或 JSONL。

## 结果与验收

每行结果包含 `case_id`、`instance_id`、基础设施 `status`、`exit_code` 和 `artifact_dir`；成功 submit 后还包含 `resolved`、`reward` 与测试计数。`resolved=false` 是有效任务结果。

```bash
sudo jq -c '{case_id,instance_id,status,resolved,reward,artifact_dir,error}' "$OUTPUT"
```

机器验收两条任务都完整执行：

```bash
sudo jq -e -s '
  length == 2 and
  (map(.instance_id) | unique | length) == 2 and
  all(.[]; .status == "completed" and (.artifact_dir | length) > 0)
' "$OUTPUT" >/dev/null && echo 'software repair evaluation completed'

while IFS= read -r artifact_dir; do
  sudo test -f "$artifact_dir/submit_result.json"
done < <(sudo jq -r '.artifact_dir' "$OUTPUT")
```

启用轨迹时，每个 completed 实例目录还应有 `trajectory_ref.json`；读取方式见[轨迹采集指南](./12-trajectory.md)。模型是否真正修复任务用 `resolved`、reward 和测试结果统计，不用 `status` 代替。

## 替换参数

| 目标 | 修改 |
|---|---|
| 模型 | `MODEL_API`、`MODEL_NAME`、provider 与密钥文件 |
| 其他实例 | 输入 JSONL 与同一 `SWE_CATALOG` |
| Agent 预算 | `--max-iterations` |
| 并发 | `--batch-size`，不超过 Gateway、容器和模型容量 |
| 离线运行 | 预先导入所有实例镜像并按发行版入口增加 `--offline` |

切换 Lite、Pro 或 Smith 不只是改一个字符串：必须同时使用匹配的 catalog、variant、输入、镜像与 workspace 规则，并先确认当前发布支持该评测 variant。Lite 和 Pro 的 catalog 用 `uenv evaluate build-swe-catalog` 从官方数据集导出生成，命令见[案例库](./02-cases.md#当前支持的环境与数据集)。

## 失败定位

| 现象 | 处理 |
|---|---|
| output 或 artifacts 已存在 | 生成新的 `RUN_ID`，不覆盖历史结果 |
| catalog 与 Gateway 不一致 | 核对同一 instance 的 repo、commit、variant 和 image key |
| 容器镜像不存在 | 预拉取/导入；离线模式不能临时访问 registry |
| completed、resolved false | 查看 patch、测试和 trajectory；这是业务未解决 |
| 并发任务互相影响 | 检查 session/workspace 隔离并降低并发 |
