# 本地凭据目录

此目录只用于在本机保存部署所需的 SSH 私钥、访问令牌和环境变量文件。除本说明外，不要提交其中的任何文件。

## 安全要求

- 不要在文档、脚本、命令行参数或日志中写入真实口令、私钥和令牌。
- 为 Server、Worker、Hub、模型服务和 SSH 分别使用独立凭据。
- 令牌文件使用最小权限，例如 `chmod 0600 TOKEN_FILE`。
- 生产主机应使用受控的 SSH key 和 `known_hosts`，不要依赖共享 root 口令。
- 怀疑凭据曾被提交或分享时，应立即轮换；仅从 Git 历史中删除并不等于凭据失效。

## 推荐文件布局

```text
secrets/
├── README.md
├── server.env        # UEnv Server 私密环境变量
├── worker.env        # UEnv Worker 私密环境变量
├── hub-reader.token  # Worker 读取 Hub 的最小权限令牌
└── deploy_ed25519    # 部署专用 SSH 私钥
```

这些文件名只是示例。实际文件必须保持未跟踪状态，并由团队的密钥管理系统或安全传输渠道分发。

## 提交前检查

```bash
git status --short
git ls-files secrets
```

第二条命令预期只列出 `secrets/README.md`。任何其他文件都应在提交前移出 Git，并轮换其中可能暴露的凭据。
