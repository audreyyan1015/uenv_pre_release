# UEnv Hub 使用指南

UEnv Hub 是可选的环境注册和制品分发服务。它管理“有哪些环境、有哪些版本、每个版本需要哪些制品”；Adapter Core 负责调度，Worker 才负责真正执行环境。

Hub 与单机/多机是两个独立维度：

| 部署形态 | 不使用 Hub | 使用 Hub |
|---|---|---|
| 单机 | 使用 release 内置或本地插件 | 注册自定义环境、锁定版本、回滚和团队共享 |
| 多机 | 人工或用其他工具保证每台 Worker 一致 | 中心发布、逐台同步并激活指定版本 |

因此，不应写成“单机不需要 Hub”或“多机必须部署 Hub”。更准确的判断是：

- 只使用固定的本地环境时，可以不使用 Hub。
- 需要环境版本、digest、团队共享、多 Worker 一致性或离线制品时，建议使用 Hub。

本文中，environment identity 是 Hub 里的环境名称和版本契约记录；EnvPackage 是 Worker 能下载的版本化制品包，可包含插件、依赖、配置和完整性 digest。前者“可查到”不等于后者已经在 Worker 上同步和激活。

## 1. Hub 的职责边界

Hub 可以：

- 保存环境 identity、语义版本、接口 Schema、配置 Schema 和资源要求。
- 保存 EnvPackage 的 manifest 和小型制品。
- 将 Proto/UDS process plugin 发布为版本化 EnvPackage。
- 同步制品时校验 SHA-256，完整后写入 `.synced` 标记。
- 原子切换 Worker 使用的插件版本。
- 下架有问题的版本，并保留显式回滚能力。
- 通过 Reader、Publisher 和 Admin Token 限制权限。

Hub 不会：

- 调度或执行 Episode。
- 替代 Adapter Core、Worker、Bridge 或训练框架。
- 保存模型权重，或自动下载完整数据集。
- 在发布新版本后主动推送到所有 Worker。
- 让正在运行的 Worker 热加载新插件；激活后仍需重启 Worker。
- 仅凭一个环境元数据 manifest 生成可执行插件。

Hub 不在 Episode 热路径上。Hub 暂时不可达不会终止已在运行的 Episode；已激活的本地插件也不会因此被删除。但新的查询、发布和同步会失败。当前 release 的 Hub 使用本机 SQLite 和本地制品目录，不自带高可用集群或跨机复制；正式使用时必须将两者纳入同一备份和恢复计划。

## 2. 单机部署 Hub

### 新服务器直接安装完整拓扑

需要 Adapter Core、Worker 和 Hub 全部位于同一台机器时，可以直接使用 `full` profile：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile full
```

默认 Hub 只监听 `127.0.0.1:8080`，且本机模式不强制 Token。这只适用于 Hub 不对其他主机提供服务的情况。

检查：

```bash
sudo systemctl is-active \
  uenv-adapter-core.service \
  uenv-worker.service \
  uenv-hub.service
curl -fsS http://127.0.0.1:8080/healthz
export UENV_HUB_ENDPOINT='http://127.0.0.1:8080'
uenv hub status
```

### 在已有单机 UEnv 上加装 Hub

如果已按 `single-node` 完成基础部署，先加装 Hub：

```bash
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile hub
```

然后备份并编辑 Worker 配置：

```bash
sudo cp -a /etc/uenv/worker.yaml \
  "/etc/uenv/worker.yaml.backup.$(date +%Y%m%d-%H%M%S)"
sudoedit /etc/uenv/worker.yaml
```

将 `hub` 段设置为：

```yaml
hub:
  enabled: true
  endpoint: "http://127.0.0.1:8080"
  token_file: ""
```

保持 `env.package_plugin_dir` 为：

```yaml
env:
  package_plugin_dir: "/var/lib/uenv/plugins"
```

验证配置并重启 Worker：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
sudo systemctl restart uenv-worker.service
```

## 3. 为多机部署一个受保护的 Hub

Hub 可以和 Adapter Core 位于同一节点，也可以使用独立节点。无论哪种方式，只要会被其他主机访问，就必须开启 Token 鉴权。

先只安装文件，不启动服务：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile hub \
  --no-start
```

生成一次性初始 Admin Token 文件。以下命令由 root 写入文件，再将它交给 `uenv` 服务账号；密钥不会打印到终端：

```bash
sudo sh -c 'umask 077; python3 -c '\''import secrets; print("uenvh_" + secrets.token_hex(32))'\'' > /etc/uenv/secrets/hub-admin.token'
sudo chown uenv:uenv /etc/uenv/secrets/hub-admin.token
sudo chmod 0600 /etc/uenv/secrets/hub-admin.token
```

用 `sudoedit /etc/uenv/hub.toml` 编辑安装器已生成的配置。只修改现有 `[server]` 和 `[auth]` 段中的以下键，保留原文件里的 `database`、`rate_limit`、`cors` 和 `packages` 等其他段：

```toml
[server]
host = "0.0.0.0"
port = 8080

[auth]
require_token = true
bootstrap_admin_token_file = "/etc/uenv/secrets/hub-admin.token"
```

`0.0.0.0` 表示所有网卡。如果操作系统支持直接绑定内网 IP，应优先填实际内网地址。同时用防火墙将 8080/TCP 只放行给 Worker、发布机和运维网络。

启动并检查：

```bash
sudo systemctl enable --now uenv-hub.service
curl -fsS http://127.0.0.1:8080/healthz
sudo journalctl -u uenv-hub.service -n 100 --no-pager
```

如果 `[server]` 的 `host` 绑定的是具体内网 IP 而不是 `0.0.0.0`，`127.0.0.1` 上不再有监听，健康检查应改用实际绑定地址，例如 `curl -fsS http://<hub-ip>:8080/healthz`。

Hub 在数据库中只保存 Token 哈希。当 Token 表为空时，上述文件用来创建初始 Admin Token。配置仍引用该文件时不要删除它，否则 Hub 重启时会因为文件不存在而失败。

## 4. 创建最小权限 Token

不要让 Worker 和发布流水线共用 Admin Token：

| 角色 | 使用者 | 权限 |
|---|---|---|
| Reader | Worker、只读验收 | 查询和下载 |
| Publisher | 环境发布者或 CI | 发布和下架环境版本；environment namespace 受 Token 限制 |
| Admin | Hub 运维 | Token、全局配置和审计 |

当前 `/packages` 的 EnvPackage 发布只检查 Publisher 角色，EnvPackage 本身没有 namespace 字段。因此 `--namespace` 可以限制 environment identity/version，但不是 EnvPackage 的多租户隔离边界。不互信的发布者不应共用同一 Hub 或 Publisher Token。

在 Hub 节点先让 root 用 Admin Token 登录。`--token-file` 会拒绝组用户或其他用户可读的文件：

```bash
sudo uenv hub login \
  --endpoint http://127.0.0.1:8080 \
  --token-file /etc/uenv/secrets/hub-admin.token
sudo uenv hub status
```

再为 Worker 和发布者分别创建最小权限 Token。`--out` 只创建新文件，权限为 `0600`，且不会在终端打印明文 Token：

```bash
sudo install -d -m 0700 /root/uenv-hub-tokens

sudo uenv hub token create \
  --name worker-reader \
  --role reader \
  --namespace default \
  --out /root/uenv-hub-tokens/worker-reader.token

sudo uenv hub token create \
  --name environment-publisher \
  --role publisher \
  --namespace default \
  --out /root/uenv-hub-tokens/environment-publisher.token
```

命令会打印 Token ID；泄露时用 `sudo uenv hub token revoke <ID>` 立即撤销。将 `worker-reader.token` 安全地分发给 Worker，将 `environment-publisher.token` 交给发布者，并在目标机保持 `0600` 权限。后续命令中的相对路径均指已安全复制到当前机器的 Token 文件。

不要把这些文件提交到 Git、不要写进普通 YAML，也不要通过无加密的公网链路传输。

Hub 原生提供 HTTP Bearer Token。如果跨越不可信网络，必须在 Hub 前使用 HTTPS 反向代理或 VPN；Token 本身不会加密 HTTP 流量。

## 5. 连接 Worker 和 CLI

安装新 Worker 时，可以直接传入 Hub 和 Reader Token 文件：

```bash
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.21:50054 \
  --hub http://10.0.0.15:8080 \
  --hub-token-file ./worker-reader.token
```

安装器会将 Token 复制为 `/etc/uenv/secrets/hub.token`，设置为 `uenv` 可读的 `0600` 文件，并在 Worker 配置中使用 `hub.token_file`。

如果 Worker 已经安装，先将 Token 安装到固定路径：

```bash
sudo install -o uenv -g uenv -m 0600 \
  ./worker-reader.token /etc/uenv/secrets/hub.token
```

然后修改 `/etc/uenv/worker.yaml`：

```yaml
hub:
  enabled: true
  endpoint: "http://10.0.0.15:8080"
  token_file: "/etc/uenv/secrets/hub.token"

env:
  # 保留原有 types、backend 和 plugin_dir，并确认同时有这一项：
  package_plugin_dir: "/var/lib/uenv/plugins"
```

旧版本生成的 Worker 配置可能没有 `env.package_plugin_dir`。缺少它时，
`uenv env sync --activate` 虽然能完成，Worker 重启后也不会扫描已激活的插件目录。
保存后先验证配置，再重启 Worker：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
sudo systemctl restart uenv-worker.service
uenv logs worker -n 100
```

发布者配置 CLI：

```bash
uenv hub login \
  --endpoint http://10.0.0.15:8080 \
  --token-file "$(realpath environment-publisher.token)"
uenv hub status
uenv env list
```

CLI 凭据保存在当前用户的 `~/.config/uenv/hub.toml`，文件权限会强制为 `0600`。`uenv hub login` 配置的是当前 CLI 用户，不会自动配置 systemd 中的 Worker；Worker 始终以 `hub.token_file` 为准。

## 6. 查询环境与版本

```bash
uenv env list
uenv env search qa
uenv env info qa
uenv env versions qa
```

这些命令查询 Hub 注册表。要查看当前节点实际已安装的插件，使用：

```bash
uenv environments
```

两者的含义不同：Hub 中有一个环境记录，不等于当前 Worker 已经安装并激活了该环境。

## 7. 发布 process plugin

先按 [UEnv 评测指南](./UEnv评测指南.md#6-环境尚未支持时怎么做) 完成本机开发和 Episode 验证。发布者登录 Hub 后，只需运行自动化入口：

```bash
bash /opt/uenv/current/examples/environment/plugin.sh publish \
  "$HOME/uenv-envs/my-environment"
```

该命令会自动运行逻辑和协议测试、创建开发 venv、下载可离线安装的依赖 wheel，并调用 Hub 发布接口。发布前它会检查：

- 目录中有 `manifest.yaml`。
- `supported_backends` 包含 `process`。
- `ipc` 是 `proto-uds`。
- `entry` 是包内的相对路径且可执行。
- 契约测试已覆盖 `HealthCheck`/`Reset`/`Step`/`Close`。
- `requirements.txt` 中的全部依赖能否放入完整 wheelhouse。

版本来自 `manifest.yaml`。行为、reward、依赖或接口变化后，先递增其中的 `version` 再重新发布。发布准备机需要访问 Python 包索引。`publish` 下载 wheelhouse 时继承系统 pip 配置：指向 HTTP 内网镜像源的配置会被 pip 以非可信主机拒绝，可用环境变量覆盖，例如 `PIP_INDEX_URL=https://mirrors.aliyun.com/pypi/simple/`。发布机还必须与目标 Worker 使用相同 Linux 架构和 Python 版本；Worker 激活时只使用包内 wheelhouse，不访问 PyPI。完全离线发布可在联网的匹配主机先运行一次 `publish` 生成 wheelhouse，再把目录带入内网并加 `--offline`。

`publish-plugin` 不会上传开发用的 `.venv`、Git 目录、Python 缓存和 `.pyc` 文件。它会发布完整 process plugin EnvPackage，并为同一 `env_type` 幂等创建 Hub environment identity 和对应版本的契约 manifest。`uenv env publish --manifest ...` 只发布环境契约和元数据；如果目标是让 Worker 运行新插件，不能只执行元数据发布。

当前 process plugin 的内联制品总大小上限为 40 MiB。模型、大型数据和容器镜像不应塞进该包，应使用对象存储、共享文件系统或私有 Registry 并锁定 digest。当前 `plugin.sh publish`/`sync --activate` 也不支持把超过该上限的 Python 依赖作为外置依赖自动安装；这类环境需预装依赖、自建运行镜像或扩展部署流程。

环境版本不可覆盖。修改了代码、判分、依赖或接口后，必须发布新版本。

## 8. 在 Worker 同步并激活

包含 Python 依赖的插件会在 Worker 上创建虚拟环境。Ubuntu/Debian Worker 在第一次激活前先执行：

```bash
sudo apt-get update
sudo apt-get install -y python3-venv
```

同步需要写入 `/var/lib/uenv`，以下示例因此为 root 用户单独配置 Reader Token。CLI 凭据按用户隔离；普通用户登录后再执行 `sudo uenv ...` 不会自动沿用该凭据。

```bash
sudo uenv hub login \
  --endpoint http://10.0.0.15:8080 \
  --token-file "$(realpath worker-reader.token)"
sudo uenv hub status

sudo uenv env sync my-environment \
  --version 0.1.0 \
  --target-dir /var/lib/uenv \
  --consumer worker \
  --worker-version 0.1.2-trial \
  --dry-run
```

确认版本、制品列表和目标节点正确后，同步并原子激活：

```bash
sudo uenv env sync my-environment \
  --version 0.1.0 \
  --target-dir /var/lib/uenv \
  --consumer worker \
  --worker-version 0.1.2-trial \
  --activate \
  --plugin-dir /var/lib/uenv/plugins

sudo systemctl restart uenv-worker.service
uenv environments
```

同步步骤会先在临时目录下载制品并逐文件校验 digest，只有完整成功后才切换版本目录。`--activate` 校验 package 是否声明 process plugin；存在 `requirements.txt` 时，它使用已校验的 `wheelhouse` 离线创建该版本的 `.venv`。依赖安装和入口校验全部成功后，才会原子切换 `/var/lib/uenv/plugins/<env_type>` 符号链接。Worker 重启时会从 `env.package_plugin_dir` 加载该插件，并上报已同步的 package ID、版本和 bundle digest。

每台 Worker 都需要显式执行同步和激活；Hub 不会将新版本主动推送到整个 Worker 集群。

每台 Worker 对同一 `env_type` 一次只能激活一个版本，切换符号链接后必须重启 Worker。仅执行 `sync --activate` 不会让所有 Episode 自动锁定该版本；集群滚动更新期间，如果请求只带 `env_type`，仍可能被调度到不同版本的 Worker。

任务 JSONL 必须同时写清环境、数据集、输入、评分方式和 package 坐标：

```json
{"id":"custom-1","env_type":"my-environment","dataset":"my-dataset","question":"Reply with exactly: ok","env_config":{"env_package_id":"my-environment","env_package_version":"0.1.0","expected_action":"ok"},"reward_config":{"type":"plugin","target":"ok"},"max_steps":1}
```

这份 JSONL 可以直接交给评测入口；命令行再次显式给出批次级路由，避免文件写错时静默落到其他环境：

```bash
uenv evaluate run-task \
  --endpoint 127.0.0.1:50051 \
  --env-type my-environment \
  --dataset my-dataset \
  --input ./my-environment.jsonl \
  --output ./my-environment-results.jsonl \
  --max-steps 1
```

VeRL 转换器会把同一行中的 `env_type`、`dataset`、`env_config`、
`reward_config` 和 `max_steps` 写入 `extra_info`，不需要用户手工编写
`sample.setdefault(...)` 补丁。完整训练命令见 UEnv 训练指南。

Adapter Core 会根据 Worker 上报的 package ID 和版本过滤调度目标。滚动期间应持续传这两个字段，直到所有 Worker 已达到同一版本。

## 9. 回滚和下架

生产任务应使用明确版本，不要把 `latest` 当成不变坐标。发布新版本的推荐顺序是：

1. 发布新版本，保留旧版本。
2. 在一台测试 Worker 上使用 `--dry-run`。
3. 同步、激活并重启该 Worker。
4. 用一个确定性 Episode 验证 reward 和 trajectory。
5. 验证通过后再逐台处理其他 Worker。

回滚时重新激活旧版本：

```bash
sudo uenv env sync my-environment \
  --version 0.1.0 \
  --target-dir /var/lib/uenv \
  --activate \
  --plugin-dir /var/lib/uenv/plugins
sudo systemctl restart uenv-worker.service
```

下架已知有问题的版本：

```bash
uenv env yank my-environment \
  --version 0.2.0 \
  --reason 'incorrect reward implementation'
```

`yank` 不会删除 Worker 已同步的文件，也不会热替换运行中的插件。需要回滚的 Worker 仍必须显式激活旧版本并重启。

## 10. 离线 Worker 如何获得容器镜像

先判断自己是否需要这一节：

| Worker 的情况 | 怎么做 |
|---|---|
| 能访问 Docker Hub 或团队私有 Registry | 跳过本节，让 Worker 正常 `pull` 镜像 |
| 不能访问任何 Registry，但能访问 UEnv Hub | 使用本节的两个命令中转镜像 |
| Hub 也访问不到 | 使用移动硬盘、内网文件服务等方式执行 `docker save/load` |

所以绝大多数联网用户都可以跳过本节。生产环境正常做法仍是把镜像推到私有 Registry，并使用固定 digest；Hub 不需要重复保存一份镜像 tar。

只有 Worker 不能访问 Registry 时，才使用 Hub 搬运镜像。用户只需要做两件事：

1. Hub 主机把已经存在的容器镜像打包并登记为一个 EnvPackage。
2. 离线 Worker 下载这个包，校验后执行 `docker load` 或 `podman load`。

这不会下载数据集，也不会安装 process plugin。镜像、任务数据和插件仍是三类独立制品。

release 提供了自动化脚本，不需要手工执行 `docker save`、复制 tar、改权限和 `docker load`。先在 Hub 主机准备好目标镜像，并为 root CLI 配置 Publisher Token，然后执行：

```bash
sudo bash /opt/uenv/current/examples/hub/image_bundle.sh publish \
  --package my-environment-images \
  --version 0.1.0 \
  --engine docker \
  --image 'registry.example.com/team/environment:1.0'
```

`--package` 是这批镜像在 Hub 中的名字，`--version` 是这批文件的版本，`--image` 是 Hub 主机上已经存在的镜像名。脚本会自动导出、设置权限并发布。镜像改变后使用新的 package 版本；正式环境建议把 `--image` 写成带 `@sha256:...` 的固定引用。

在离线 Worker 上，为 root CLI 配置 Reader Token 后执行：

```bash
sudo bash /opt/uenv/current/examples/hub/image_bundle.sh install \
  --package my-environment-images \
  --version 0.1.0 \
  --engine docker \
  --target-dir /var/lib/uenv \
  --worker-version 0.1.2-trial
```

这条命令会从 Hub 下载 tar、校验文件并自动导入 Docker。使用 Podman 时加 `--engine podman`；脚本会把镜像导入运行 Worker 的 `uenv` 用户所使用的 rootless Podman 存储，而不是 root 自己的存储。前提是管理员已经为 `uenv` 系统用户配置好 rootless Podman（包括 subuid/subgid 和运行时目录），否则脚本会在导入前明确停止。同一个 package/version 在每台离线 Worker 上各执行一次即可。

Hub 在这里充当的是“受校验的文件中转站”，不是容器 Registry，也不会主动把新镜像推送到每台 Worker。每台离线 Worker 都要明确执行一次 `install`。

大型数据集和模型权重不要使用这个镜像脚本，也不要塞进 40 MiB 的 plugin package。它们应放在对象存储、共享文件系统或专用模型仓库中，由环境配置记录路径和 digest；本节只解决“离线 Worker 缺少容器镜像”这一件事。

## 11. 备份、恢复和排障

可恢复的 Hub 备份必须同时包含：

- SQLite 数据库 `/var/lib/uenv/hub/hub.db` 及对应 WAL。
- 制品目录 `/var/lib/uenv/hub/artifacts`。
- `/etc/uenv/hub.toml` 和 Token 恢复计划。

仅备份 SQLite 不足以恢复 EnvPackage 制品。建议在备份前停止 Hub，将数据库与 artifacts 作为同一个一致性快照，完成后再启动并检查 `/healthz`。

常用排障命令：

```bash
curl -fsS http://127.0.0.1:8080/healthz
uenv hub status
uenv env list
uenv logs hub -n 200
sudo journalctl -u uenv-hub.service -n 200 --no-pager
```

常见问题：

- `401 Unauthorized`：CLI Token、Worker `token_file` 或 Token 角色不正确。
- Hub 健康但 Worker 连接失败：检查 Hub 监听地址、防火墙和 Worker 视角的 URL。
- Hub 中能查到环境，Worker 却不能执行：检查是否发布了完整 plugin package，是否执行了 `uenv env sync --activate`，以及 Worker 是否重启。
- 激活时无法创建 `.venv`：确认 Worker 已安装 `python3-venv`，并检查 wheelhouse 是否与该 Worker 的 Python 和系统平台匹配。
- 同步中断：重试前查看错误；未完成的临时目录不会被激活。
- 回滚后行为未变：确认 `/var/lib/uenv/plugins/<env_type>` 指向的版本，并重启 Worker。

`uenv hub sync` 只用于列出 Hub 元数据变更，不会同步或激活 Worker 制品。真正的节点制品同步命令是 `uenv env sync <package> ...`。
