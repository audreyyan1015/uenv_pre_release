# UEnv 配置目录说明
#
# 本地开发
#   uenv-worker.yaml / uenv-worker.json     Worker 通用示例（math）
#   uenv-mock-scheduler.yaml                  Mock ControlPlane
#   uenv-worker.swe-local.yaml                本机 SWE Gateway 联调（:38999）
#   server.yaml                               uenv-server / adapter-core
#
# 实机部署（按机器）
#   uenv-server.deploy.yaml                   75.157 Server（gRPC :8088 + 轨迹 :8077）
#   uenv-server.trajectory.env.example        Server 轨迹环境变量模板
#   uenv-worker.deploy-7143.yaml              7143 四端 math Worker
#   uenv-worker.deploy-7143.standby.yaml      Server 未就绪时本机 standby
#   uenv-worker.deploy-7143-swe-pro.yaml      7143 SWE Pro + Gateway :28097
#   uenv-worker.deploy-7143.env.example       7143 环境变量模板
#   uenv-trajectory.env.example               Worker/OpenHands 轨迹上传模板
#   openhands-20877.env.example               208.77 OpenHands 环境变量
#   openhands-llm-20877.json.example          208.77 LLM JSON 模板
#   uenv-llm-gateway-7142.yaml.example        7142 LLM 网关
#   uenv-worker-llm.env.example               Worker/OpenHands LLM（勿提交 *.env）
#
# SWE catalog（Hub seed + Worker 回退）
#   swe/verified.json                         Verified 样例
#   swe/pro.json                              Pro 实例（export 脚本生成）
#   swe/pro-python-smoke.json                 OpenHands smoke 单实例
#   swe-default-config.json                   Hub default_config 结构示例（单测引用）
