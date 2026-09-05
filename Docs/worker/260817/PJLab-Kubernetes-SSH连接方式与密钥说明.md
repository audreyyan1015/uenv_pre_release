# 浦江 SSH 连接方式与密钥说明

## 1. 连接方式

当前本机通过浦江代理的 SOCKS5 端口连接 SSH 网关：

```bash
ssh -vvv \
  -i ~/.ssh/id_ed25519_pjlab \
  -o IdentitiesOnly=yes \
  -o IdentityAgent=none \
  -o 'ProxyCommand=nc -x 127.0.0.1:7890 -X 5 %h %p' \
  'ws-ea44e548183dfd98-worker-5mfwj.fangtianshun+root.ailab-sys.pod@h.pjlab.org.cn'
```

说明：

```text
SSH 网关：h.pjlab.org.cn
SSH 端口：22
本地 SOCKS5：127.0.0.1:7890
```

SSH 不会自动使用 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 环境变量，必须通过
`ProxyCommand` 显式指定 SOCKS5 转发。

## 2. 密钥位置

```text
私钥：~/.ssh/id_ed25519_pjlab
公钥：~/.ssh/id_ed25519_pjlab.pub
```

私钥权限要求：

```bash
chmod 600 ~/.ssh/id_ed25519_pjlab
```

公钥指纹：

```text
SHA256:cxLmHxJFIlwmuR0nRQqo0fslAjZILrhpEO4eJ2zFQL4
```

管理员只需要公钥，不需要私钥。私钥不得发送、提交到 Git 或复制到服务器。

## 3. 当前验证结果

2026-08-19 通过上述 SOCKS5 代理和本地私钥验证成功：

```text
Server accepts key
Authenticated to h.pjlab.org.cn (via proxy) using "publickey"
Exit status: 0
```

本次测试只执行了 `exit`，未执行其他远程命令，也未修改服务器或集群资源。
