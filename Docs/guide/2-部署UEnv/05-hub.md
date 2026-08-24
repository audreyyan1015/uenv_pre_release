# 部署和使用 UEnv Hub

UEnv Hub 用于保存环境版本和 EnvPackage，并支持把指定版本预同步到一台或多台 UEnv Worker。团队需要共享自定义环境、统一多台 UEnv Worker 的环境版本、离线预置环境包或回滚旧版本时，可以部署 UEnv Hub。

一条任务样本在 UEnv 中执行一次，称为一个 Episode。UEnv Server 负责分配 Episode，UEnv Worker 负责执行环境，UEnv Hub 负责登记、保存和分发环境版本（UEnv Server 服务的代码名是 `uenv-adapter-core`）。

如果当前只使用 UEnv 安装包内置的环境，先完成[单机部署](./01-single-node.md)和[通用评测流程](../3-运行任务/03-evaluation.md)即可。需要共享或统一管理环境版本时，再按照本页部署 UEnv Hub。

| 当前需求 | 是否需要 UEnv Hub |
|---|---|
| 一台 UEnv Worker 使用 UEnv 安装包内置环境 | 可以直接使用当前安装，无需增加 UEnv Hub |
| 多台 UEnv Worker 预同步同一个自定义环境版本 | 使用 UEnv Hub |
| 记录环境版本、校验文件内容或回滚旧版本 | 使用 UEnv Hub |
| 离线 UEnv Worker 需要从内网下载 EnvPackage | 使用内网 UEnv Hub |

## 本页使用的名词

| 名词 | 含义 |
|---|---|
| UEnv Hub | 保存环境名称、版本、接口说明和 EnvPackage 的服务 |
| EnvPackage（环境包） | 一个确定版本的环境文件集合，供 UEnv Worker 预同步、激活或按运行时配置加载 |
| process plugin（进程插件） | 在 UEnv Worker 上以独立进程运行的环境插件 |
| 内容摘要（digest） | 用于核对文件内容是否一致的校验值 |
| 容器镜像仓库（OCI Registry） | 保存容器镜像的服务，例如 Docker Hub 或团队的私有镜像仓库 |
| UEnv Hub 访问令牌（Token） | UEnv Hub 用于识别访问者及其权限的文件 |

发布并预置一个 process plugin EnvPackage 包含以下步骤：

1. 开发者创建并测试 process plugin。
2. 发布者把该版本作为 EnvPackage 发布到 UEnv Hub。
3. 管理员在目标 UEnv Worker 上预同步并激活该 EnvPackage。
4. 管理员重启 UEnv Worker，使其加载新版本。

发布见[发布 process plugin 的 EnvPackage](#发布-process-plugin-的-envpackage)，预同步和激活见[在 UEnv Worker 上预同步并激活 EnvPackage](#在-uenv-worker-上预同步并激活-envpackage)。这个流程属于任务前的运维准备：Episode 运行时由 UEnv Server 选择 Worker 并下发任务，调度与结果返回始终只在 UEnv Server 与 UEnv Worker 之间进行。

## 选择部署方式

| 当前部署 | 执行的小节 |
|---|---|
| 新主机同时运行 UEnv Server、UEnv Worker 和 UEnv Hub | [在新主机上安装全部组件](#在新主机上安装全部组件) |
| 现有单机部署需要增加 UEnv Hub | [为现有单机部署增加 UEnv Hub](#为现有单机部署增加-uenv-hub) |
| UEnv Hub 使用独立主机 | [使用独立的 UEnv Hub 主机](#使用独立的-uenv-hub-主机) |

三种方式提供相同的 UEnv Hub 功能，每次部署只执行对应的小节：

- 前两种方式下，UEnv Hub 默认只供本机访问，且默认关闭令牌鉴权。完成后直接继续[发布](#发布-process-plugin-的-envpackage)与[预同步](#在-uenv-worker-上预同步并激活-envpackage)。
- 独立主机方式下，UEnv Hub 通过网络服务多台主机。安装完成后，继续[创建访问令牌](#创建访问令牌)和[让 UEnv Worker 连接 UEnv Hub](#让-uenv-worker-连接-uenv-hub)。

### 在新主机上安装全部组件

`full` 安装模式（profile）会在同一台主机安装 UEnv Server、UEnv Worker 和 UEnv Hub：

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

### 为现有单机部署增加 UEnv Hub

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

### 使用独立的 UEnv Hub 主机

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

只有新数据库且 token 表为空时，服务才会用该文件创建 bootstrap Admin。配置仍引用该文件时不要删除它。

启动并检查 UEnv Hub：

```bash
sudo systemctl enable --now uenv-hub.service
curl -fsS http://10.0.0.15:8080/healthz
sudo journalctl -u uenv-hub.service -n 100 --no-pager
```

防火墙只向 UEnv Worker、发布主机和运维主机开放 `8080/TCP`。跨越不受信任的网络时，在 UEnv Hub 前配置 HTTPS 反向代理或 VPN。

## 创建访问令牌

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

生成的令牌文件权限为 `0600`。`token create --out` 不覆盖已存在的文件：目标文件已存在时会报 `File exists` 错误，需要先移走或删除旧文件再重新创建。把只读令牌安全复制到每台 UEnv Worker 主机，把发布者令牌安全复制到发布主机。

## 让 UEnv Worker 连接 UEnv Hub

### 安装新的 UEnv Worker

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

`--server` 是 UEnv Server 的 gRPC 地址；`--advertise` 是 UEnv Server 回连这台 UEnv Worker 的地址。

### 配置已经安装的 UEnv Worker

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

### 配置发布主机

在发布主机使用发布者令牌登录：

```bash
uenv hub login \
  --endpoint http://10.0.0.15:8080 \
  --token-file "$(realpath environment-publisher.token)"
uenv hub status
uenv env list
```

当前用户的 UEnv Hub 登录配置保存在 `~/.config/uenv/hub.toml`。UEnv Worker 的 UEnv Hub 连接配置保存在 `/etc/uenv/worker.yaml`。

## 发布 process plugin 的 EnvPackage

先创建并测试 process plugin（接口模板见[自定义环境](../3-运行任务/11-process-plugin.md)）。测试通过后，在发布主机执行：

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

## 在 UEnv Worker 上预同步并激活 EnvPackage

发布把 EnvPackage 保存到 UEnv Hub。接下来在每台目标 UEnv Worker 上分别预同步它。这一步适合离线 Worker、多 Worker 固定版本和灰度/回滚准备；日常 Episode 执行不经过这一步。

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
| `--activate` | 是 | 下载、校验，并把 process plugin 设为当前激活版本 |

`--dry-run` 是可选检查：

```bash
sudo uenv env sync my-environment \
  --version 0.1.0 \
  --target-dir /var/lib/uenv \
  --consumer worker \
  --worker-version 0.1.2-trial \
  --dry-run
```

process plugin 需要作为本机可执行环境加载时，使用 `--activate`：

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

`--activate` 会下载 EnvPackage、核对内容摘要、安装 Python 依赖，并把该 process plugin 版本设置为当前激活版本。所有步骤成功后，UEnv Worker 才切换到新版本。重启服务后，UEnv Worker 启动时加载该版本，并在注册信息中上报已同步的 EnvPackage。

多台 UEnv Worker 需要逐台执行以上预同步、激活和重启操作。更新期间，可以在任务样本的 `env_config` 中加入以下两个字段，指定 EnvPackage：

```json
{
  "env_package_id": "my-environment",
  "env_package_version": "0.1.0"
}
```

UEnv Server 会把该任务样本交给已经上报此 EnvPackage 版本的 UEnv Worker。Worker 收到 Server 下发的 Episode 后，再从本机实例池获取或按需拉起环境实例；如果实例已存在则复用，如果缺实例则按需创建。

SWE 这类运行时包不使用 `--activate` 激活 process plugin。同步后将 Worker 配置中的 `swe.env_package_dir`、`swe.env_package_dirs` 或环境变量 `UENV_SWE_ENV_PACKAGE` 指向本地 EnvPackage 目录。Worker 启动后从该目录读取 catalog、overlay 和镜像索引；Episode 到达时再由 SWE 实例池按需 provision 对应实例。

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

## 环境、process plugin 和容器镜像的关系

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

## 回滚或停止使用某个版本

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

## 备份和排障

完整备份应同时包含：

- 数据库 `/var/lib/uenv/hub/hub.db`，以及同目录中名称以 `hub.db-` 开头的配套文件。
- EnvPackage 文件目录 `/var/lib/uenv/hub/artifacts`。
- 配置 `/etc/uenv/hub.toml`。
- 管理员令牌文件和权限记录。

备份前停止 `uenv-hub.service`。复制以上文件后重新启动服务，并检查 `/healthz`。

注意：`uenv-hub.service` 的工作目录固定为 `/var/lib/uenv/hub`。若要清空数据重新初始化（例如重新 bootstrap 管理员令牌），不能直接删除该目录——服务会以 `200/CHDIR` 启动失败。正确做法是停止服务后清空目录内容，并以 `install -o uenv -g uenv -m 0755 -d /var/lib/uenv/hub` 重建空目录后再启动。

常用检查命令（UEnv Hub 与检查主机同机、且监听回环地址时）：

```bash
curl -fsS http://127.0.0.1:8080/healthz
uenv hub status
uenv env list
uenv logs hub -n 200
sudo journalctl -u uenv-hub.service -n 200 --no-pager
```

独立 UEnv Hub（[使用独立的 UEnv Hub 主机](#使用独立的-uenv-hub-主机)）把 `127.0.0.1` 换成 `hub.toml` 中配置的监听地址，例如 `curl -fsS http://10.0.0.15:8080/healthz`。`uenv doctor` 的 Hub 健康检查同样使用 `hub.toml` 的监听地址。

| 现象 | 检查内容 |
|---|---|
| `401 Unauthorized` | UEnv Hub 访问令牌、令牌角色和 UEnv Worker 的 `token_file` |
| UEnv Hub 健康，但 UEnv Worker 无法连接 | UEnv Hub 监听地址、防火墙和 UEnv Worker 配置的 URL |
| UEnv Hub 已保存版本，但 UEnv Worker 找不到环境 | process plugin 检查 `sync --activate` 结果、插件目录和 UEnv Worker 重启状态；SWE 检查本地 EnvPackage 目录配置 |
| Python 虚拟环境创建失败 | `python3-venv`、Python 离线依赖目录、Python 版本和 CPU 架构 |
| 回滚后行为未改变 | `/var/lib/uenv/plugins/<env_type>` 当前版本和 UEnv Worker 重启状态 |

`uenv hub sync` 只查看 UEnv Hub 注册信息的变化。预同步 EnvPackage 使用 `uenv env sync <package> ...`；process plugin 需要加载为可执行环境时再加 `--activate`。
