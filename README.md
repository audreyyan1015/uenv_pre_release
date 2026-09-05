# UEnv — 面向评测与强化学习训练的环境执行系统

UEnv 让评测程序和强化学习框架通过同一套接口运行环境任务：

```text
评测程序 / 强化学习框架 → UEnv Bridge → UEnv Server → UEnv Worker
```

- **UEnv Bridge** 把框架中的任务样本转换成 UEnv 请求，并把结果交还框架。
- **UEnv Server** 接收任务、选择可用的 Worker，并汇总执行结果。
- **UEnv Worker** 运行环境、调用模型、计算回报并记录轨迹。
- **UEnv Hub** 是可选服务，用于管理和分发环境版本；不参与每次任务的执行链路。

一条任务样本的一次环境执行称为一个 **Episode**。你不需要先理解内部 RPC、调度租约或源码目录，就可以完成部署、评测和训练。

## 从文档开始

正式用户手册从 [UEnv 使用手册](./Docs/guide/1-了解UEnv/01-index.md) 开始。推荐按下面的顺序了解完整能力：

1. [架构与组件](./Docs/guide/1-了解UEnv/02-architecture.md)
2. [单机部署](./Docs/guide/2-部署UEnv/01-single-node.md)
3. [多机部署](./Docs/guide/2-部署UEnv/02-multi-node.md)
4. [通用评测流程](./Docs/guide/3-运行任务/03-evaluation.md)
5. [强化学习训练指南](./Docs/guide/3-运行任务/07-post-training.md)
6. [获取轨迹](./Docs/guide/3-运行任务/12-trajectory.md)
7. [案例库](./Docs/guide/3-运行任务/02-cases.md)
8. [运行维护](./Docs/guide/5-运维UEnv/01-operations.md)

单机部署用于理解和验证基础拓扑；多机部署继续说明独立 Server、Worker 注册和扩容。评测与强化学习训练是两项并列的核心能力，都在主路径中完整介绍。

按需查阅：

- [配置 UEnv Server](./Docs/guide/2-部署UEnv/03-server.md)
- [配置并注册 UEnv Worker](./Docs/guide/2-部署UEnv/04-worker-registration.md)
- [部署和使用 UEnv Hub](./Docs/guide/2-部署UEnv/05-hub.md)
- [自定义强化学习框架接入](./Docs/guide/4-接入强化学习框架/01-custom-framework.md)
- [故障排查](./Docs/guide/5-运维UEnv/02-troubleshooting.md)
- [术语表](./Docs/guide/6-查阅参考/01-glossary.md)

部署阶段只检查服务健康、Worker 注册状态和双向网络，不额外提交业务任务。

## 单机安装

准备发布包中的三个文件：

```text
install.sh
uenv-linux-x86_64.tar.gz
uenv-linux-x86_64.tar.gz.sha256
```

在安装主机执行：

```bash
INSTALL_DIR="$HOME/uenv-install"
cd "$INSTALL_DIR"
sha256sum -c uenv-linux-x86_64.tar.gz.sha256
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile single-node
```

安装完成后检查：

```bash
sudo -u uenv uenv doctor
sudo systemctl is-active uenv-adapter-core.service
sudo systemctl is-active uenv-worker.service
curl -fsS http://127.0.0.1:50052/health
curl -fsS http://127.0.0.1:19090/health
uenv status
```

当前安装包中的 UEnv Server 服务名是 `uenv-adapter-core.service`。这是需要在命令中使用的服务名；其他兼容名称见[术语表](./Docs/guide/6-查阅参考/01-glossary.md)。

预期结果是两个服务均为 `active`，两个健康接口成功，并且 `uenv status` 显示一台状态为 `ready` 的 Worker。

## 支持的任务类型

`env_type` 选择环境执行方式，`dataset` 选择该环境中的数据或判分规则。

| `env_type` | 示例 | 用途 |
|---|---|---|
| `qa` | `gsm8k`、`pubmedqa`、`scitab` | 问答、分类和规则判分 |
| `code` | 代码生成示例 | 生成代码并运行测试 |
| `swe` | `verified`、`lite`、`pro`、`smith` | 软件工程任务与 Agent 执行 |

团队自己的环境可以通过 process plugin 接入。案例输入是教学示例，不代表对应公开数据集的完整评测结果。

## 开发者入口

以下内容面向维护 UEnv 源码的开发者，普通部署和使用不要求阅读。

| 路径 | 用途 |
|---|---|
| `proto/` | Bridge、Server 和 Worker 的 gRPC 协议 |
| `plugin_proto/` | Worker 与 process plugin 的本机协议 |
| `uenv-bridge/` | 强化学习框架侧 Bridge 实现 |
| `uenv-bridge/core/` | 当前 UEnv Server 可执行程序的源码目录 |
| `uenv-server/` | Server 使用的内部调度库 |
| `uenv-worker/` | Worker 运行时 |
| `uenv-hub/` | 可选的环境版本服务 |
| `examples/cases/` | 评测和训练示例输入 |
| `templates/` | process plugin 模板 |

### 从源码构建发布包

构建主机需要 x86_64 Linux、Rust stable、Cargo、C/C++ 构建工具、OpenSSL 开发头文件、`pkg-config`、`protoc`、Python 3.10+ 与 pip。

```bash
git clone https://github.com/audreyyan1015/uenv_pre_release.git
cd uenv_pre_release

python3 -m venv .uenv-build-venv
. .uenv-build-venv/bin/activate
python -m pip install --upgrade pip wheel

bash -n install.sh scripts/build-release.sh
python3 -m unittest tests.test_installation_assets -v
bash ./scripts/build-release.sh --version 0.1.2-trial
```

构建产物位于 `dist/`：

```text
dist/install.sh
dist/uenv-linux-x86_64.tar.gz
dist/uenv-linux-x86_64.tar.gz.sha256
```

### 开发验证

```bash
cargo test -p uenv-server
cargo test -p uenv-worker
cargo test -p uenv-adapter-core
python3 -m unittest tests.test_installation_assets -v
```

协议定义以 `proto/`、`uenv-worker/proto/` 和 `plugin_proto/` 为准。

## 许可

Apache-2.0
