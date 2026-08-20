# 运行维护

本页面向已经完成部署的运维人员，覆盖状态检查、配置变更、扩容、升级、下线和备份。任务正确性仍由评测、强化学习训练或案例结果判断。

## 日常检查

在 UEnv Server 主机：

```bash
uenv status
uenv workers
curl -fsS http://127.0.0.1:50052/health
uenv logs server -n 200
```

在 UEnv Worker 主机：

```bash
curl -fsS http://127.0.0.1:19090/health
uenv logs worker -n 200
```

正常状态是 UEnv Server 和 UEnv Worker 健康请求成功、预期 UEnv Worker 为 `ready`、endpoint 与容量正确。持续查看日志时使用 `uenv logs server -f` 或 `uenv logs worker -f`。

## 安全修改配置

1. 在维护窗口开始前记录 `uenv status` 和 `uenv workers`。
2. 备份 `/etc/uenv`。
3. 只修改目标组件的配置。
4. 运行该组件的配置校验。
5. 只重启被修改的服务，并检查恢复状态。

在对应主机执行：

```bash
BACKUP_DIR=/var/backups/uenv
STAMP="$(date +%Y%m%d-%H%M%S)"
sudo install -d -m 0700 "$BACKUP_DIR"
sudo cp -a /etc/uenv "${BACKUP_DIR}/etc-uenv-${STAMP}"

sudo -u uenv env UENV_CONFIG_PATH=/etc/uenv/server.yaml \
  /opt/uenv/current/bin/uenv-adapter-core --validate-config
sudo -u uenv /opt/uenv/current/bin/uenv-worker \
  --config /etc/uenv/worker.yaml validate-config
```

只在实际修改了对应组件时重启：

```bash
sudo systemctl restart uenv-adapter-core.service
sudo systemctl restart uenv-worker.service
```

UEnv Server 重启后等待 UEnv Worker 自动重新注册，再确认数量、endpoint、容量和 `ready` 状态。校验失败时不要重启；恢复备份配置并再次校验。

## 扩容 UEnv Worker

扩容沿用[多机部署](../2-部署UEnv/02-multi-node.md)的 UEnv Worker 安装步骤：使用同版本安装包，填写 UEnv Server 地址和新 UEnv Worker 的公布地址，再验证 `50051/TCP`、`50054/TCP` 和注册状态。

不要只看 UEnv Worker 服务是否启动。扩容完成标志是 UEnv Server 上的 `uenv workers` 出现新节点，状态为 `ready`，环境能力和容量符合预期。

## 滚动升级 UEnv Worker

逐台处理，避免同时损失全部执行容量：

1. 在 UEnv Server 主机运行 `uenv workers`，确认目标 UEnv Worker 当前负载为 0，没有活动 Episode。
2. 停止目标 UEnv Worker：`sudo systemctl stop uenv-worker.service`。
3. 保留旧安装包和 `/etc/uenv` 备份，再使用新安装包重复原 UEnv Worker 安装命令。
4. 检查 `uenv version`、服务健康、两个网络方向和重新注册。
5. 该 UEnv Worker 恢复 `ready` 后，再处理下一台。

当前发布 CLI 没有单独的 drain 子命令。目标 UEnv Worker 仍有负载时不要强制替换；先停止产生新任务并等待负载归零。若无法归零，先定位卡住的 Episode，不要删除结果 WAL 来强行清空状态。

## 升级 UEnv Server

先完成并验证 UEnv Worker 的兼容升级，再升级 UEnv Server：

1. 停止提交新任务，等待所有 UEnv Worker 当前负载归零。
2. 记录 `uenv status`，备份 `/etc/uenv` 和 `/var/lib/uenv/server`。
3. 保留旧安装包，使用新安装包重复 `--profile control-plane` 安装。
4. 检查 `uenv version`、Admin 健康接口和 UEnv Server 日志。
5. 等待 UEnv Worker 自动重新注册，核对数量、endpoint、容量和状态。

如果新版本验证失败，停止继续升级其他节点。使用保留的旧发布包和升级前配置执行回退；配置格式发生变化时先确认旧版本仍能读取，不要直接覆盖唯一备份。

## 下线 UEnv Worker

在 UEnv Server 主机确认目标 UEnv Worker 当前负载为 0，再到 UEnv Worker 主机执行：

```bash
sudo systemctl disable --now uenv-worker.service
```

停止心跳后，UEnv Server 默认约 30 秒暂停把新任务调度到该记录。之后该记录在 `uenv status` / `uenv workers` 中显示为 `degraded`（约 90 秒后达到观测离线阈值），但记录本身不会自动删除，仍计入 `Worker=N` 并长期保留；当前版本没有删除单条 Worker 记录的命令。固定 UEnv Worker ID 的替换实例必须等旧实例停止后再启动。

## 备份与恢复

| 组件 | 必须保留 |
|---|---|
| UEnv Server | `/etc/uenv/server.yaml`、`server.env`、`/var/lib/uenv/server/`、相关密钥 |
| UEnv Worker | `/etc/uenv/worker.yaml`、`worker.env`、`/var/lib/uenv/worker/wal/`、环境包、相关密钥 |
| UEnv Hub | `/etc/uenv/hub.toml`、UEnv Hub token、`/var/lib/uenv/hub/` |

SQLite 数据库必须与 WAL/SHM 文件一致。最简单的可靠方式是在维护窗口停止对应服务，再复制完整数据目录；不要只复制主数据库文件。

以 UEnv Server 为例，在 UEnv Server 主机执行：

```bash
BACKUP_DIR=/var/backups/uenv
STAMP="$(date +%Y%m%d-%H%M%S)"
ARCHIVE="${BACKUP_DIR}/server-${STAMP}.tar.gz"

sudo install -d -m 0700 "$BACKUP_DIR"
sudo systemctl stop uenv-adapter-core.service
sudo tar -C / -czf "$ARCHIVE" \
  etc/uenv/server.yaml \
  etc/uenv/server.env \
  var/lib/uenv/server
sudo chmod 0600 "$ARCHIVE"
sudo systemctl start uenv-adapter-core.service
sudo systemctl is-active uenv-adapter-core.service
curl -fsS http://127.0.0.1:50052/health
uenv workers
```

如果归档命令失败，后续启动命令仍会恢复服务；先检查失败原因并重新完成备份，不要把失败归档当作可恢复备份。恢复操作会覆盖运行数据，应在独立维护窗口按目标版本的恢复流程执行，并先验证备份副本。

遇到异常时转到[故障排查](./02-troubleshooting.md)。
