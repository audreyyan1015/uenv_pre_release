# uenv-hub — UEnv 环境注册中心

UEnvHub 是 UEnv 分布式环境框架的**持久化环境注册中心**，属于离线目录服务，不参与运行时调度。Worker 启动时从 Hub 拉取环境定义和元数据。

## 职责

- **环境元数据管理**：存储环境类型、版本、描述、资源需求等信息
- **镜像索引**：维护环境镜像地址和摘要的映射
- **版本管理**：基于 semver 的版本控制和兼容性追踪
- **环境发现**：支持搜索和查询已注册的环境

## 架构

```
┌────────────────────────────────────────────┐
│  uenv-hub                                   │
│                                              │
│  gRPC (port 50053)                          │
│  ┌──────────────────────────────────────┐   │
│  │ HubService                            │   │
│  │   GetEnvDefinition / SearchEnv /      │   │
│  │   PublishEnv                          │   │
│  └──────────────────────────────────────┘   │
│                                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐   │
│  │ 元数据   │ │ 存储    │ │ 版本管理 │   │
│  │ 索引     │ │ Git/S3  │ │ semver   │   │
│  └──────────┘ └──────────┘ └──────────┘   │
└────────────────────────────────────────────┘
```

## 四级注册链路

| 级别 | 执行者 | 职责 |
|:-----|:-------|:------|
| UEnvHub 发布 | 环境开发者 | 推送环境元数据（名称/版本/资源需求/镜像地址） |
| Worker 本地注册 | Worker 启动 | @register_env 加载环境类到本地注册表 |
| Server 全局注册 | UEnv Server | Worker 上报 supported_envs，更新全局路由表 |
| 元数据同步 | UEnv Server | 从 Hub 拉取元数据充实注册表 |

## gRPC 服务

| RPC | 说明 |
|:----|:------|
| GetEnvDefinition | 获取指定环境的定义和工件 |
| SearchEnv | 搜索已注册的环境 |
| PublishEnv | 发布新环境或新版本 |

## 数据模型

```protobuf
message EnvMeta {
    string env_type = 1;
    string version = 2;
    string description = 3;
    ResourceSpec resources = 4;
    repeated string backends = 5;
    string image_url = 6;
    string config_schema = 7;
}
```

## 快速使用

```bash
# 启动
cargo run -- start --port 50053

# 发布环境
cargo run -- push math-env 1.0.0

# 搜索环境
cargo run -- search math
```

## 配置

参考 ../config/hub.example.toml：

```toml
port = 50053

[storage]
backend = "git"
path = "./env-index"
```

## 存储后端演进

| Phase | 后端 | 说明 |
|:------|:-----|:------|
| Phase 2 | Git + YAML | 轻量级，YAML manifest |
| Phase 3 | HTTP API | REST API + 对象存储 |
| Phase 4 | 生态化 | Web UI + 评分 + 社区发布 |

## 依赖

- **通信**: tonic (gRPC), prost (Protobuf)
- **运行时**: tokio
