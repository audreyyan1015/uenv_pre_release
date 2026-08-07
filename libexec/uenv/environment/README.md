# 自定义环境入口

`plugin.sh` 把 process plugin 的常用操作收敛为四个命令：

```bash
./plugin.sh create my-environment --dataset my-dataset
# 编辑 my-environment/environment.py
./plugin.sh test my-environment
sudo ./plugin.sh install-local my-environment
./plugin.sh publish my-environment
```

安装后仍使用与 QA、Code 相同的显式任务入口：

```bash
uenv evaluate run-task \
  --endpoint 127.0.0.1:50051 \
  --env-type my-environment \
  --dataset my-dataset \
  --input ./my-environment/example.jsonl \
  --output ./my-environment/results.jsonl \
  --max-steps 1
```

运行 `./plugin.sh --help` 查看离线模式、目标目录和发布参数。创建后的目录中还会有
一份完整 README，说明 `reset`、`step`、`reward` 与 Episode 配置的数据结构。
