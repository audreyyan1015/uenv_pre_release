# 910C VeRL/vLLM smoke 排障记录

> 日期：2026-08-13
> 目标：记录本机 Ascend 910C/9392 环境下 VeRL、vLLM-Ascend、torch-npu 的 smoke 结果和排障结论，避免后续重复走弯路。

## 1. 当前结论

本机 worker 容器内已经安装 VeRL、vLLM 和 vLLM-Ascend，入口级 smoke 可以通过：

- `/usr/local/python3.11.13/bin/python` 能导入 `verl`、`vllm`、`vllm_ascend`、`torch_npu`
- `/usr/local/python3.11.13/bin/vllm serve --help` 能激活 Ascend platform plugin
- `python -m verl.trainer.main_ppo --help` 能进入 VeRL/MindSpeed 参数解析

但当前镜像不能真正执行基础 NPU 算子。最小 `torch_npu` op 失败：

```text
torch.zeros((8,), device="npu")
RuntimeError: call aclnnInplaceZero failed, error code is 561103
Parse dynamic kernel config fail
ZerosLike ADD_TO_LAUNCHER_LIST_AICORE failed
```

因此当前状态应判断为：

```text
VeRL/vLLM/vLLM-Ascend 包已安装并能加载；
NPU 设备可见；
但 CANN/OPP/torch-npu 动态 kernel 运行时不可用；
在修复 torch.zeros(device="npu") 前，不应继续排查 adapter 或 VeRL 训练逻辑。
```

## 2. 必须使用的 Python 和环境

不要用宿主默认 `/usr/bin/python3` 做判断。当前可用环境是：

```bash
/usr/local/python3.11.13/bin/python
/usr/local/python3.11.13/bin/vllm
```

当前包版本：

```text
torch       2.7.1
torch_npu   2.7.1
verl        0.8.0.dev        (/home/verl/verl)
vllm        0.11.0+empty     (/home/vllm/vllm)
vllm_ascend 0.11.0           (/home/vllm-ascend/vllm_ascend)
```

做任何 smoke 前先设置：

```bash
source /usr/local/Ascend/ascend-toolkit/set_env.sh
source /usr/local/Ascend/nnal/atb/set_env.sh
export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
export TORCH_DEVICE_BACKEND_AUTOLOAD=0
export ASCEND_VISIBLE_DEVICES=0
export ASCEND_RT_VISIBLE_DEVICES=0
export VLLM_USE_V1=1
```

说明：

- 不补 driver lib 时，`vllm` CLI 可能因 `libascend_hal.so` 找不到而失败。
- 不设置 `TORCH_DEVICE_BACKEND_AUTOLOAD=0` 时，导入链路可能报 `Only a single TORCH_LIBRARY can be used to register the namespace triton`。
- VeRL/Ray 任务中还应透传 `RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=1`，避免 Ray 改写 Ascend 可见卡。

## 3. 硬件和 CANN 现状

`npu-smi` 能看到 16 张 Ascend910 芯片，当前健康且空闲。单卡 board 信息示例：

```text
NPU Name       : 9392
Chip Name      : Ascend910
Chip Version   : V1
Firmware       : 7.8.0.2.212
```

当前 CANN/ATB：

```text
CANN toolkit/runtime/opp/opp_kernel: 8.3.RC1 / 8.3.0.1.200
ATB: 8.3.RC1.B130
```

OPP 目录中可以看到 `Ascend910_9392` 的部分 op 数据：

```text
/usr/local/Ascend/ascend-toolkit/8.3.RC1/opp/built-in/data/op/Ascend910_9392/
```

但动态 kernel RL 配置只看到：

```text
dynamic_Ascend910A_AiCore_32_v001.json
dynamic_Ascend910A_AiCore_32_v002.json
dynamic_Ascend910ProB_AiCore_30_v001.json
```

没有看到 `Ascend910_9392` 或明确 910C 对应的 dynamic JSON。结合错误里的 `Parse dynamic kernel config fail`，这是当前最可疑点。

## 4. 最小复现命令

### 4.1 torch_npu 基础算子

```bash
bash -lc '
source /usr/local/Ascend/ascend-toolkit/set_env.sh >/dev/null 2>&1
source /usr/local/Ascend/nnal/atb/set_env.sh >/dev/null 2>&1
export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
export TORCH_DEVICE_BACKEND_AUTOLOAD=0
export ASCEND_VISIBLE_DEVICES=0
export ASCEND_RT_VISIBLE_DEVICES=0
/usr/local/python3.11.13/bin/python - <<PY
import torch, torch_npu
print("torch", torch.__version__)
print("torch_npu", torch_npu.__version__)
print("npu available", torch.npu.is_available())
print("npu device count", torch.npu.device_count())
x = torch.zeros((8,), device="npu")
print(x)
PY
'
```

当前失败点：

```text
zero_: call aclnnInplaceZero failed, error code is 561103
Parse dynamic kernel config fail
ZerosLike ADD_TO_LAUNCHER_LIST_AICORE failed
```

补充实验：

```text
torch.empty((8,), device="npu")  可以分配
torch.rand((8,), device="npu")   可以返回
torch.zeros(...)                 失败
torch.ones(...)                  失败
torch.empty(...) + 1             失败
```

换 `ASCEND_VISIBLE_DEVICES=0/1/8` 后现象一致，因此不像单卡硬件故障。

### 4.2 vLLM-Ascend CLI

```bash
bash -lc '
source /usr/local/Ascend/ascend-toolkit/set_env.sh >/dev/null 2>&1
source /usr/local/Ascend/nnal/atb/set_env.sh >/dev/null 2>&1
export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
export TORCH_DEVICE_BACKEND_AUTOLOAD=0
/usr/local/python3.11.13/bin/vllm --version
/usr/local/python3.11.13/bin/vllm serve --help | head -80
'
```

通过标准：

```text
Platform plugin ascend is activated
usage: vllm serve ...
```

当前该项可以通过。

### 4.3 vLLM dummy server

本机有一个只有 config/tokenizer、没有权重的目录：

```text
/data/uenvspace/geruijun/Qwen3-32B
```

可用它和 `--load-format dummy` 做服务初始化 smoke，不依赖真实权重：

```bash
bash -lc '
source /usr/local/Ascend/ascend-toolkit/set_env.sh >/dev/null 2>&1
source /usr/local/Ascend/nnal/atb/set_env.sh >/dev/null 2>&1
export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
export TORCH_DEVICE_BACKEND_AUTOLOAD=0
export ASCEND_VISIBLE_DEVICES=0
export ASCEND_RT_VISIBLE_DEVICES=0
export VLLM_USE_V1=1
timeout 90s /usr/local/python3.11.13/bin/vllm serve /data/uenvspace/geruijun/Qwen3-32B \
  --served-model-name Qwen3-32B-dummy \
  --host 127.0.0.1 --port 18095 \
  --max-model-len 128 \
  --tensor-parallel-size 1 \
  --load-format dummy \
  --dtype float32 \
  --disable-log-stats
'
```

当前能走到：

```text
Platform plugin ascend is activated
Resolved architecture: Qwen3ForCausalLM
device_config=npu
PIECEWISE compilation enabled on NPU
Initializing a V1 LLM engine
```

随后失败在：

```text
/home/vllm-ascend/vllm_ascend/worker/model_runner_v1.py
self.input_ids = torch.zeros(...)
RuntimeError: aclnnInplaceZero failed, error code is 561103
```

这说明 vLLM-Ascend 软件层已经加载，阻断点是底层 `torch_npu` 基础算子。

### 4.4 VeRL 入口

```bash
bash -lc '
source /usr/local/Ascend/ascend-toolkit/set_env.sh >/dev/null 2>&1
source /usr/local/Ascend/nnal/atb/set_env.sh >/dev/null 2>&1
export LD_LIBRARY_PATH=/usr/local/Ascend/driver/lib64/driver:${LD_LIBRARY_PATH:-}
export TORCH_DEVICE_BACKEND_AUTOLOAD=0
/usr/local/python3.11.13/bin/python -m verl.trainer.main_ppo --help | head -80
'
```

当前可以进入 help 输出和 MindSpeed patch 日志。完整训练仍依赖上面的 NPU 基础算子修复。

## 5. 根因判断

当前最合理判断是：

```text
CANN 8.3.RC1 + torch_npu 2.7.1 + 当前 OPP/opp_kernel 包
对本机 9392/910C 的 ACLNN 动态 kernel 配置不完整或版本不匹配。
```

原因：

1. `torch.npu.is_available()` 为 True，设备枚举正常。
2. `vllm_ascend` plugin 正常激活，vLLM 能解析 Qwen3 架构并进入 NPU engine 初始化。
3. `torch.zeros/ones/add` 这类基础算子在 ACLNN 动态 kernel 解析阶段失败。
4. 同一错误在多张卡上复现，排除单卡故障概率较高。
5. OPP 里缺少明显对应 9392/910C 的 dynamic RL JSON。
6. vLLM-Ascend 0.11.0 官方安装文档要求的是 CANN 8.3.RC2 和 `torch-npu 2.7.1.post1`，当前镜像是 CANN 8.3.RC1 和 `torch_npu 2.7.1`。

## 6. 最短修复路径

优先不要继续改 adapter，也不要只调 vLLM 参数。先让这个命令通过：

```bash
python - <<'PY'
import torch, torch_npu
x = torch.zeros((8,), device="npu")
print(x)
PY
```

建议顺序：

1. 优先使用官方或基准 vLLM-Ascend 0.11.0 镜像验证。
   目标组合：
   - CANN 8.3.RC2
   - torch 2.7.1
   - torch-npu 2.7.1.post1
   - vllm-ascend 0.11.0
   - 对应 Ascend-cann-kernels-910b 8.3.RC2
2. 如果必须基于当前镜像，做派生镜像，不建议直接覆盖当前容器。
   - `FROM 当前镜像`
   - 替换或升级 CANN toolkit/runtime/opp/opp_kernel 到 8.3.RC2
   - 替换或升级 NNAL/ATB 到匹配版本
   - 安装 `torch-npu==2.7.1.post1`
   - 保留 `/home/verl`、`/home/vllm`、`/home/vllm-ascend` 源码
3. 不建议优先尝试：
   - 修改 adapter 代码
   - 改 VeRL agent loop 逻辑
   - 只调 `--dtype`、`--enforce-eager`、`--load-format`
   - 只重复 source toolkit/ATB/opp_kernel

通过标准：

```text
torch.zeros(device="npu") 通过
vllm dummy server 能启动
真实小模型 vLLM OpenAI 请求能返回
VeRL 小 step smoke 能进入 rollout 和训练
```

## 7. 项目侧已做的 runner 调整

为避免项目脚本再次踩环境变量问题，已经在 Ascend 分支补齐：

- `UENV_DEVICE_BACKEND=ascend`
- `ASCEND_VISIBLE_DEVICES`
- `ASCEND_RT_VISIBLE_DEVICES`
- `TORCH_DEVICE_BACKEND_AUTOLOAD=0`
- `RAY_EXPERIMENTAL_NOSET_ASCEND_RT_VISIBLE_DEVICES=1`
- 容器内 source `/usr/local/Ascend/ascend-toolkit/set_env.sh`
- 容器内 source `/usr/local/Ascend/nnal/atb/set_env.sh`
- 容器内补 `/usr/local/Ascend/driver/lib64/driver` 到 `LD_LIBRARY_PATH`
- Ray runtime env 透传上述关键变量

相关文件：

```text
uenv-bridge/scripts/train/launchers/common/run_verl_uenv_grpo.sh
uenv-bridge/scripts/train/launchers/swe/ascend/swe_smith_grpo_train_ascend.sh
libexec/uenv/training/verl_runner.sh
```

这些调整只能保证项目启动时使用正确 Ascend 环境，不能修复当前 CANN/OPP 动态 kernel 缺失或版本不匹配。

## 8. 参考资料

| 来源 | 说明 |
|---|---|
| https://docs.vllm.ai/projects/ascend/en/v0.11.0/installation.html | vLLM-Ascend 0.11.0 安装要求，包含 CANN 8.3.RC2、torch-npu 2.7.1.post1、kernels |
| https://github.com/vllm-project/vllm-ascend/issues/5695 | vLLM-Ascend 同类 `aclnnInplaceZero / 561103 / Parse dynamic kernel config fail` |
| https://github.com/vllm-project/vllm-ascend/issues/4859 | vLLM-Ascend 同类 engine 初始化失败 |
| https://gitee.com/ascend/MindSpeed-LLM/issues/IBHHBV | MindSpeed 场景中 `torch.zeros(device="npu")` 同类失败 |
| https://www.hiascend.com/document/detail/en/CANNCommunityEdition/900/API/aolapi/atlasascendc_api_07_1205.html | ACLNN 错误码说明 |
| https://www.hiascend.com/dev/forum/thread-02177215788123559300-1-1.html | 华为论坛对 dynamic kernel config fail 的排查方向 |
