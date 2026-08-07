# UEnv Hub 使用指南

UEnv Hub 用于保存环境版本，并把指定版本安装到一台或多台 UEnv Worker。团队需要共享自定义环境、统一多台 UEnv Worker 的环境版本或回滚旧版本时，可以部署 UEnv Hub。

一条任务样本在 UEnv 中执行一次，称为一个 Episode。Adapter 负责分配 Episode，UEnv Worker 负责执行环境，UEnv Hub 负责登记、保存和分发环境版本。Adapter 由 `uenv-adapter-core.service` 运行，其内部使用 UEnv Server（`uenv-server`）模块完成 UEnv Worker 注册、Episode 调度和状态管理。

如果当前只使用 UEnv 安装包内置的环境，可以先完成基础部署和评测。需要共享或统一管理环境版本时，再按照本指南部署 UEnv Hub。

| 当前需求 | 是否需要 UEnv Hub |
|---|---|
| 一台 UEnv Worker 使用 UEnv 安装包内置环境 | 可以直接使用当前安装，无需增加 UEnv Hub |
| 多台 UEnv Worker 安装同一个自定义环境版本 | 使用 UEnv Hub |
| 记录环境版本、校验文件内容或回滚旧版本 | 使用 UEnv Hub |
| 离线 UEnv Worker 需要从内网下载 EnvPackage | 使用内网 UEnv Hub |

## 1. 本指南使用的名词

| 名词 | 含义 |
|---|---|
| UEnv Hub | 保存环境名称、版本、接口说明和 EnvPackage 的服务 |
| EnvPackage（环境包） | 一个确定版本的环境文件集合，供 UEnv Worker 下载和安装 |
| process plugin（进程插件） | 在 UEnv Worker 上以独立进程运行的环境插件 |
| 内容摘要（digest） | 用于核对文件内容是否一致的校验值 |
| 容器镜像仓库（OCI Registry） | 保存容器镜像的服务，例如 Docker Hub 或团队的私有镜像仓库 |
| UEnv Hub 访问令牌（Token） | UEnv Hub 用于识别访问者及其权限的文件 |

发布并使用一个 process plugin 包含以下步骤：

1. 开发者创建并测试 process plugin。
2. 发布者把该版本作为 EnvPackage 发布到 UEnv Hub。
3. 管理员在目标 UEnv Worker 上下载并激活该 EnvPackage。
4. 管理员重启 UEnv Worker，使其加载新版本。

第 5 节介绍发布，第 6 节介绍下载和激活。

## 2. 选择 UEnv Hub 的部署方式

| 当前部署 | 执行的小节 |
|---|---|
| 新主机需要同时运行 Adapter、UEnv Worker 和 UEnv Hub | 2.1 |
| 现有单机 UEnv 需要增加 UEnv Hub | 2.2 |
| UEnv Hub 使用独立主机 | 2.3 |

三种方式提供相同的 UEnv Hub 功能。每次部署只执行对应的小节。

- 选择 2.1 或 2.2 时，UEnv Hub 默认只供本机访问，且默认关闭令牌鉴权。完成对应小节后，直接继续第 5、6 节。
- 选择 2.3 时，UEnv Hub 通过网络服务多台主机。完成 2.3 后，继续第 3、4 节配置访问令牌和 UEnv Worker。

### 2.1 在新主机上安装全部组件

`full` 安装模式（profile）会在同一台主机安装 Adapter、UEnv Worker 和 UEnv Hub：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile full
```

检查三个服务：

```bash
sudo systemctl is-active \
  uenv-adapter-core.service \
  uenv-worker.service \
  uenv-hub.service
curl -fsS http://127.0.0.1:8080/healthz
export UENV_HUB_ENDPOINT='http://127.0.0.1:8080'
uenv hub status
```

默认情况下，UEnv Hub 只监听 `127.0.0.1:8080`。本机的 `uenv` 命令和本机 UEnv Worker 可以直接访问它。

### 2.2 为现有单机部署增加 UEnv Hub

现有主机已经使用 `single-node` 安装模式时，运行：

```bash
sudo bash install.sh \
  --bundle /home/uenv-install/uenv-linux-x86_64.tar.gz \
  --profile hub
```

使用 `sudoedit /etc/uenv/worker.yaml` 修改现有配置。将现有 `hub` 段改为下列值，并确认现有 `env` 段包含 `package_plugin_dir`：

```yaml
hub:
  enabled: true
  endpoint: "http://127.0.0.1:8080"
  token_file: ""

env:
  package_plugin_dir: "/var/lib/uenv/plugins"
```

保留 `env` 段中的 `types`、`backend`、`plugin_dir` 等其他现有字段。配置中只保留一个 `hub` 段和一个 `env` 段。

验证配置并重启 UEnv Worker：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
sudo systemctl restart uenv-worker.service
```

### 2.3 使用独立的 UEnv Hub 主机

以下示例把 UEnv Hub 部署在 `10.0.0.15:8080`。先安装文件，并暂时保持服务停止：

```bash
cd /home/uenv-install
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile hub \
  --no-start
```

创建初始管理员令牌：

```bash
sudo sh -c 'umask 077; python3 -c '\''import secrets; print("uenvh_" + secrets.token_hex(32))'\'' > /etc/uenv/secrets/hub-admin.token'
sudo chown uenv:uenv /etc/uenv/secrets/hub-admin.token
sudo chmod 0600 /etc/uenv/secrets/hub-admin.token
```

编辑 `/etc/uenv/hub.toml`：

```toml
[server]
host = "10.0.0.15"
port = 8080

[auth]
require_token = true
bootstrap_admin_token_file = "/etc/uenv/secrets/hub-admin.token"
```

启动并检查 UEnv Hub：

```bash
sudo systemctl enable --now uenv-hub.service
curl -fsS http://10.0.0.15:8080/healthz
sudo journalctl -u uenv-hub.service -n 100 --no-pager
```

防火墙只向 UEnv Worker、发布主机和运维主机开放 `8080/TCP`。跨越不受信任的网络时，在 UEnv Hub 前配置 HTTPS 反向代理或 VPN。

## 3. 创建 UEnv Hub 访问令牌

UEnv Hub 提供三种令牌角色：

| 角色 | 交给谁 | 可以执行的操作 |
|---|---|---|
| 只读令牌（reader） | UEnv Worker、只读检查人员 | 查询和下载 |
| 发布者令牌（publisher） | 环境发布人员或 CI | 发布和下架环境版本 |
| 管理员令牌（admin） | UEnv Hub 管理员 | 创建令牌和执行全部管理操作 |

在 UEnv Hub 主机使用管理员令牌登录，然后创建另外两种令牌：

```bash
sudo uenv hub login \
  --endpoint http://10.0.0.15:8080 \
  --token-file /etc/uenv/secrets/hub-admin.token

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

生成的令牌文件权限为 `0600`。把只读令牌安全复制到每台 UEnv Worker 主机，把发布者令牌安全复制到发布主机。

## 4. 让 UEnv Worker 连接 UEnv Hub

### 4.1 安装新的 UEnv Worker

安装时直接提供 UEnv Hub 地址和只读令牌：

```bash
sudo bash install.sh \
  --bundle ./uenv-linux-x86_64.tar.gz \
  --profile worker \
  --server 10.0.0.10:50051 \
  --advertise 10.0.0.21:50054 \
  --hub http://10.0.0.15:8080 \
  --hub-token-file ./worker-reader.token
```

`--server` 是 Adapter 的 gRPC 地址。参数名中的 `server` 是为了与现有配置兼容，对应 Adapter 内部的 UEnv Server 模块。`--advertise` 是 Adapter 访问这台 UEnv Worker 的 gRPC 地址。

### 4.2 配置已经安装的 UEnv Worker

先安装只读令牌：

```bash
sudo install -o uenv -g uenv -m 0600 \
  ./worker-reader.token /etc/uenv/secrets/hub.token
```

使用 `sudoedit /etc/uenv/worker.yaml` 修改现有配置。将现有 `hub` 段改为下列值，并确认现有 `env` 段包含 `package_plugin_dir`：

```yaml
hub:
  enabled: true
  endpoint: "http://10.0.0.15:8080"
  token_file: "/etc/uenv/secrets/hub.token"

env:
  package_plugin_dir: "/var/lib/uenv/plugins"
```

保留 `env` 段中的其他现有字段。配置中只保留一个 `hub` 段和一个 `env` 段。

验证配置并重启 UEnv Worker：

```bash
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
sudo systemctl restart uenv-worker.service
```

### 4.3 配置发布主机

在发布主机使用发布者令牌登录：

```bash
uenv hub login \
  --endpoint http://10.0.0.15:8080 \
  --token-file "$(realpath environment-publisher.token)"
uenv hub status
uenv env list
```

当前用户的 UEnv Hub 登录配置保存在 `~/.config/uenv/hub.toml`。UEnv Worker 的 UEnv Hub 连接配置保存在 `/etc/uenv/worker.yaml`。

## 5. 发布 process plugin 的 EnvPackage

先按照 [UEnv 评测指南](./UEnv评测指南.md#6-接入新任务) 创建并测试 process plugin。测试通过后，在发布主机执行：

```bash
uenv env plugin publish "$HOME/uenv-envs/my-environment"
```

该命令会：

1. 检查插件清单、运行文件和 UEnv 插件接口。
2. 执行环境逻辑测试和接口测试。
3. 收集 Python 依赖。
4. 创建 EnvPackage，并把版本和内容摘要登记到 UEnv Hub。

修改环境行为、得分计算、依赖或接口后，先增加 `manifest.yaml` 中的 `version`，再发布新版本。同一个名称和版本发布后保持内容不变。

发布主机和目标 UEnv Worker 应使用相同的 Linux CPU 架构与 Python 版本。联网发布会从 Python 包索引下载依赖，下载沿用发布主机的 pip 配置；使用 HTTP 内网镜像时，`trusted-host` 需要写在 pip 配置的 `[global]` 段（只写在 `[install]` 段时发布阶段的依赖下载会失败）。离线发布需要提前准备 Python 离线依赖目录（wheelhouse），并增加 `--offline`。

该命令直接上传的文件总量上限为 40 MiB。模型权重、大型官方数据集和容器镜像分别保存在模型存储、数据存储和容器镜像仓库中。

## 6. 在 UEnv Worker 上下载并激活 EnvPackage

第 5 节把 EnvPackage 保存到 UEnv Hub。接下来在每台目标 UEnv Worker 上分别安装它。

process plugin 包含 Python 依赖时，先安装 `python3-venv`：

```bash
sudo apt-get update
sudo apt-get install -y python3-venv
```

独立且启用令牌鉴权的 UEnv Hub 需要先让 root 用户使用只读令牌登录：

```bash
sudo uenv hub login \
  --endpoint http://10.0.0.15:8080 \
  --token-file "$(realpath worker-reader.token)"
sudo uenv hub status
```

UEnv Hub 与 UEnv Worker 同机、并使用默认本机配置时，跳过以上登录命令。后面的同步命令会使用默认地址 `http://127.0.0.1:8080`。

两种同步选项的作用如下：

| 选项 | 是否修改 UEnv Worker | 作用 |
|---|---|---|
| `--dry-run` | 否 | 显示将下载的文件，并检查版本兼容性 |
| `--activate` | 是 | 下载、校验、安装并设为当前使用版本 |

`--dry-run` 是可选检查：

```bash
sudo uenv env sync my-environment \
  --version 0.1.0 \
  --target-dir /var/lib/uenv \
  --consumer worker \
  --worker-version 0.1.2-trial \
  --dry-run
```

实际安装使用 `--activate`：

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

`--activate` 会下载 EnvPackage、核对内容摘要、安装 Python 依赖，并设置当前版本。所有步骤成功后，UEnv Worker 才切换到新版本。重启服务后，UEnv Worker 开始使用该版本。

多台 UEnv Worker 需要逐台执行以上安装、激活和重启操作。更新期间，可以在任务样本的 `env_config` 中加入以下两个字段，指定 EnvPackage：

```json
{
  "env_package_id": "my-environment",
  "env_package_version": "0.1.0"
}
```

Adapter 会把该任务样本交给已经加载此 EnvPackage 版本的 UEnv Worker。

查看 UEnv Hub 中的版本：

```bash
uenv env list
uenv env search qa
uenv env info qa
uenv env versions qa
```

查看当前主机已加载的环境：

```bash
uenv environments
```

## 7. 环境、process plugin 和容器镜像的关系

环境规定任务输入、交互步骤和得分计算方式。process plugin 是环境的一种实现方式。容器镜像提供任务运行所需的文件系统和软件依赖。

| 环境 | UEnv Worker 的执行方式 | 容器镜像要求 |
|---|---|---|
| QA、Code 或普通自定义环境 | 启动 process plugin | 通常无容器镜像 |
| SWE | UEnv Worker 中的 SWE Runtime 根据 SWE catalog 启动 SWE 实例镜像 | SWE 实例镜像 |
| 自定义容器环境 | 由该环境的专用运行组件启动容器 | 由环境实现规定 |

容器镜像仓库提供镜像上传和拉取接口。联网的 UEnv Worker 使用 Docker 或 Podman，从容器镜像仓库拉取镜像。UEnv Hub 保存环境版本、镜像引用和内容摘要。

无法访问容器镜像仓库、但可以访问内网 UEnv Hub 的 UEnv Worker，可以通过 UEnv Hub 下载镜像 tar。先在已经保存目标镜像的 UEnv Hub 主机执行：

```bash
sudo bash /opt/uenv/current/tools/hub/image_bundle.sh publish \
  --package my-environment-images \
  --version 0.1.0 \
  --engine docker \
  --image 'registry.example.com/team/environment:1.0'
```

然后在目标 UEnv Worker 主机执行：

```bash
sudo bash /opt/uenv/current/tools/hub/image_bundle.sh install \
  --package my-environment-images \
  --version 0.1.0 \
  --engine docker \
  --target-dir /var/lib/uenv \
  --worker-version 0.1.2-trial
```

第一条命令把镜像导出为 tar，并将其发布为镜像 EnvPackage。第二条命令下载、校验并导入该镜像。使用 Podman 时把参数改为 `--engine podman`，并先确认 `uenv` 账户可以运行 Podman。

process plugin EnvPackage 和镜像 EnvPackage 当前需要分别发布和安装。环境配置或 SWE catalog 中的镜像引用负责指定要使用的镜像版本。

## 8. 回滚或停止使用某个版本

回滚时，在目标 UEnv Worker 上重新激活旧版本并重启服务：

```bash
sudo uenv env sync my-environment \
  --version 0.1.0 \
  --target-dir /var/lib/uenv \
  --activate \
  --plugin-dir /var/lib/uenv/plugins
sudo systemctl restart uenv-worker.service
```

把有问题的版本标记为不可再选择：

```bash
uenv env yank my-environment \
  --version 0.2.0 \
  --reason 'incorrect reward implementation'
```

`yank` 只影响后续选择。已经安装该版本的 UEnv Worker 仍需执行回滚命令。

## 9. 备份和排障

完整备份应同时包含：

- 数据库 `/var/lib/uenv/hub/hub.db`，以及同目录中名称以 `hub.db-` 开头的配套文件。
- EnvPackage 文件目录 `/var/lib/uenv/hub/artifacts`。
- 配置 `/etc/uenv/hub.toml`。
- 管理员令牌文件和权限记录。

备份前停止 `uenv-hub.service`。复制以上文件后重新启动服务，并检查 `/healthz`。

常用检查命令（UEnv Hub 与检查主机同机、且监听回环地址时）：

```bash
curl -fsS http://127.0.0.1:8080/healthz
uenv hub status
uenv env list
uenv logs hub -n 200
sudo journalctl -u uenv-hub.service -n 200 --no-pager
```

独立 UEnv Hub（第 2.3 节）把 `127.0.0.1` 换成 `hub.toml` 中配置的监听地址，例如 `curl -fsS http://10.0.0.15:8080/healthz`。`uenv doctor` 的 Hub 健康检查同样使用 `hub.toml` 的监听地址。

| 现象 | 检查内容 |
|---|---|
| `401 Unauthorized` | UEnv Hub 访问令牌、令牌角色和 UEnv Worker 的 `token_file` |
| UEnv Hub 健康，但 UEnv Worker 无法连接 | UEnv Hub 监听地址、防火墙和 UEnv Worker 配置的 URL |
| UEnv Hub 已保存版本，但 UEnv Worker 找不到环境 | `sync --activate` 结果、process plugin 目录和 UEnv Worker 重启状态 |
| Python 虚拟环境创建失败 | `python3-venv`、Python 离线依赖目录、Python 版本和 CPU 架构 |
| 回滚后行为未改变 | `/var/lib/uenv/plugins/<env_type>` 当前版本和 UEnv Worker 重启状态 |

`uenv hub sync` 只查看 UEnv Hub 注册信息的变化。下载和激活 EnvPackage 使用 `uenv env sync <package> ...`。
