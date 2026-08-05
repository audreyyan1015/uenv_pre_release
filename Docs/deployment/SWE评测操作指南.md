# UEnv SWE 评测操作指南

本文只讲模型评测，不讲 VeRL 训练。完成后，你会用一个模型运行一条真实的 SWE-bench Verified 任务，
并从 `submit_result.json` 查看 reward 和测试结果。

支持两种模型来源：

- 火山引擎方舟 API：UEnv 主机不需要 GPU，建议第一次先走这条路径；
- 本地模型服务：使用已经部署好的 vLLM、SGLang 或其他 OpenAI-compatible `/v1` API。

## 1. 评测前要有什么

UEnv 主机必须已经按
[UEnv SWE 使用入口](./SWE评测与VeRL训练操作指南.md)
完成 `--enable-swe` 安装。先检查：

```bash
sudo systemctl is-active uenv-adapter-core.service uenv-worker.service
curl -fsS http://127.0.0.1:28999/runtime/v1/health
sudo runuser -u uenv -- docker info >/dev/null
test -s /opt/uenv/current/share/swe/verified.json
```

默认评测实例是 `astropy__astropy-7166`。release 只包含这类任务的元数据，不包含完整数据集，也不包含
该实例的 Docker 镜像。`evaluate.sh` 首次运行时会按需下载：

```text
swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest
```

## 2. 安装 OpenHands

在 UEnv 主机执行一次：

```bash
sudo bash /opt/uenv/current/examples/swe/install_openhands.sh
```

该命令会从 GitHub 下载 release 固定提交的 OpenHands benchmarks 和 SDK，安装 Python 3.12 及冻结的
Python 依赖。它不会下载模型。

检查安装结果：

```bash
sudo test -x /opt/uenv/agent/openhands-benchmarks/.venv/bin/python
echo "OpenHands 已安装"
```

如果 UEnv 主机不能访问 GitHub 或 Python 包源，应先解决网络或代理问题。不要自行替换为未经本 release
验证的 OpenHands 最新提交。

## 3. 路径 A：使用火山引擎方舟 API

你需要自行在方舟控制台准备：

- API Key；
- 推理接入点 ID，例如 `ep-xxxxxxxx`；
- 足够的调用额度。

评测最多会进行 30 次 Agent 迭代，实际调用次数和费用取决于模型行为。第一次可以增加
`--max-iterations 5` 检查链路，再使用默认值做完整尝试。

### 3.1 运行一次低迭代检查

评测脚本需要读取 root 可见的 Gateway 密钥，因此入门命令在 root shell 中执行：

```bash
sudo -i
mkdir -p /var/tmp/uenv-eval/results
cd /var/tmp/uenv-eval

read -rsp 'ARK_API_KEY: ' ARK_API_KEY
export ARK_API_KEY
echo
export ARK_ENDPOINT_ID='ep-xxxxxxxx'

bash /opt/uenv/current/examples/swe/evaluate.sh volcengine \
  --model "$ARK_ENDPOINT_ID" \
  --max-iterations 5 \
  --output-dir "$PWD/results/ark-check"

unset ARK_API_KEY
```

首次运行可能在“下载实例镜像”阶段停留较久。另开终端可以查看：

```bash
sudo runuser -u uenv -- docker images
df -h /var/lib/docker
```

### 3.2 查看结果

仍在 root shell 中执行：

```bash
test -s /var/tmp/uenv-eval/results/ark-check/submit_result.json
cat /var/tmp/uenv-eval/results/ark-check/submit_result.json
exit
```

如果链路检查成功，需要给模型完整操作空间时，重新运行并去掉 `--max-iterations 5`。

脚本默认使用：

```text
https://ark.cn-beijing.volces.com/api/v3
```

账号所在区域或代理要求其他地址时，在调用前设置 `ARK_BASE_URL`。

## 4. 路径 B：使用本地模型服务

UEnv 评测脚本不会下载模型权重，也不会替你启动模型服务。开始本节前，必须已经有一个可用的
OpenAI-compatible API。

模型服务可以：

- 与 UEnv 位于同一台 GPU 主机，此时通常使用 `127.0.0.1`；
- 位于另一台 GPU 主机，此时使用该机器的受控内网地址。

模型及服务需要支持 OpenAI Chat Completions 和 OpenHands 使用的工具调用。文档中的模型名只是参数示例，
不代表该模型在你的 GPU 和服务配置下已经通过效果验收。

### 4.1 从 UEnv 主机检查模型 API

先设置地址并查询模型列表：

```bash
export LOCAL_MODEL_BASE_URL='http://127.0.0.1:8000/v1'
curl -fsS "$LOCAL_MODEL_BASE_URL/models"
```

如果模型在另一台机器，例如：

```bash
export LOCAL_MODEL_BASE_URL='http://10.0.0.20:8000/v1'
curl -fsS "$LOCAL_MODEL_BASE_URL/models"
```

确保模型服务监听的是 UEnv 主机可达的地址，并只向可信内网或 VPN 开放端口。记录 `/v1/models` 返回的
模型 ID；后面的 `--model` 必须原样使用这个 ID。

如果模型 API 需要密钥，必须在运行评测的同一个 root shell 中设置。下一节的命令会先询问密钥；不需要
密钥时直接按回车即可。

### 4.2 运行评测

```bash
sudo -i
mkdir -p /var/tmp/uenv-eval/results
cd /var/tmp/uenv-eval

export LOCAL_MODEL_NAME='the-id-returned-by-v1-models'
export LOCAL_MODEL_BASE_URL='http://127.0.0.1:8000/v1'

read -rsp 'LOCAL_MODEL_API_KEY（无密钥直接回车）: ' LOCAL_MODEL_API_KEY
echo
if [[ -n "$LOCAL_MODEL_API_KEY" ]]; then
  export LOCAL_MODEL_API_KEY
else
  unset LOCAL_MODEL_API_KEY
fi

bash /opt/uenv/current/examples/swe/evaluate.sh local \
  --model "$LOCAL_MODEL_NAME" \
  --base-url "$LOCAL_MODEL_BASE_URL" \
  --output-dir "$PWD/results/local"

unset LOCAL_MODEL_API_KEY 2>/dev/null || true
test -s "$PWD/results/local/submit_result.json"
cat "$PWD/results/local/submit_result.json"
exit
```

模型 API 不需要密钥时，不设置 `LOCAL_MODEL_API_KEY`，脚本会使用 `EMPTY`。

## 5. 脚本实际做了什么

无论使用哪种模型，`evaluate.sh` 都按相同顺序执行：

1. 检查 Worker Runtime Gateway；
2. 从 Verified catalog 查找 `--instance`；
3. 根据实例确定 Docker 镜像；
4. 镜像不存在时，以 Worker 使用的 `uenv` 用户拉取；
5. 创建只在本次运行使用的模型配置文件；
6. 启动 OpenHands，调用模型并操作 UEnv 环境；
7. 提交结果，让 Worker 执行测试；
8. 在 `--output-dir` 写入结果。

模型 API Key 只从环境变量读取；临时模型配置使用 `0600` 权限并在运行结束时删除。

## 6. 怎样判断评测成功

链路成功至少满足：

```bash
test -s /path/to/output/submit_result.json
```

重点查看：

- `reward`：任务得分；
- `tests_passed`：通过测试数；
- `tests_total`：测试总数。

`evaluate.sh` 正常退出但 `reward` 为 0，表示模型没有解决这道题，不表示 UEnv 运行失败。容器创建、模型
认证或测试执行失败时，脚本会返回非零退出码并打印对应阶段的错误。

## 7. 切换实例与数据集边界

查看 release 内置的 10 个实例：

```bash
python3 - /opt/uenv/current/share/swe/verified.json <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as stream:
    catalog = json.load(stream)
for instance_id in sorted(catalog):
    print(instance_id)
PY
```

选择另一个内置实例时增加：

```text
--instance another-instance-id
```

脚本每次评测一个实例。需要使用自定义任务清单时，同时传入：

```text
--catalog /absolute/path/to/verified-catalog.json
--instance instance-id-in-that-catalog
```

官方完整数据集见 <https://huggingface.co/datasets/SWE-bench/SWE-bench_Verified>。当前 release 没有完整
数据集下载和转换命令；Hugging Face 原始目录不能直接作为 `--catalog`。自定义 catalog 必须转换为以
`instance_id` 为键、包含题目、仓库版本和测试信息的 UEnv JSON，并准备对应 Docker 镜像。

不要在首次试用时拉取所有 benchmark 镜像。需要批量运行时，应先制定实例列表、磁盘预算和失败重试策略。

## 8. 离线运行

如果镜像已经提前导入，可以给评测命令增加：

```text
--offline
```

此时缺少实例镜像会立即失败，不会访问镜像仓库。导入后用 Worker 用户检查：

```bash
sudo runuser -u uenv -- docker image inspect \
  swebench/sweb.eval.x86_64.astropy_1776_astropy-7166:latest
```

`--offline` 只控制实例镜像下载。OpenHands、模型权重和其他依赖仍需提前准备。

## 9. 常见问题

### Gateway 不可用

```bash
sudo systemctl status uenv-worker.service --no-pager
sudo journalctl -u uenv-worker.service -n 200 --no-pager
sudo ss -lntp | grep 28999
curl -v http://127.0.0.1:28999/runtime/v1/health
```

### `uenv` 用户不能拉取镜像

```bash
id uenv
sudo runuser -u uenv -- docker info
sudo runuser -u uenv -- docker pull 'the-image-from-the-error-message'
```

### 模型 API 连接失败或返回 401

先从 UEnv 主机重新检查 `/v1/models`。确认 URL、模型 ID 和 API Key 均属于同一个模型服务。远程模型端口
应同时检查云安全组、主机防火墙和模型服务监听地址。

### OpenHands 安装失败

```bash
sudo bash /opt/uenv/current/examples/swe/install_openhands.sh
sudo test -x /opt/uenv/agent/openhands-benchmarks/.venv/bin/python
```

检查 GitHub、Python 包源、系统时间和磁盘空间。

## 10. 什么时候需要 Hub

本指南使用单个 Worker 的本地 catalog，评测不需要 UEnv Hub。只有多个 Worker 需要同步同一批环境版本、
镜像和元数据时，才建议接入 Hub。
