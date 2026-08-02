#!/usr/bin/env python3
"""分布式压测公共运行时工具。

这个文件实现跨机器压测会反复用到的基础能力：解析 worker 节点清单，建立 SSH 连接，把 worker 分配到不同主机，检查端口是否空闲，采集源码和二进制清单，读取远端进程信息，并保护已经在线的正式 adapter-core 不被本次压测误停或误替换。

实现逻辑是：命令行入口通过 add_runtime_arguments 注入统一参数，再由 configure_from_args 建立 server 与 worker 主机连接；启动前用 protected_snapshot 记录正式进程和端口状态，用 assert_port_free/assert_ports_free 避免占用已有服务；压测期间 start_owned 只启动带有本次 run_id 标识的进程，stop_owned 只停止这些自有进程；压测结束后 assert_protected_unchanged 再次比对正式进程，确认隔离运行没有影响线上服务。"""

from __future__ import annotations

import argparse
import base64
import dataclasses
import hashlib
import json
import re
import shlex
import time

import paramiko


# 75.157 作为隔离 server 机器；65.20/51.129 作为真实 worker / OpenHands 容器机器。
SERVER_HOST = "8.130.75.157"
WORKER_HOST = "8.130.65.20"
SECONDARY_WORKER_HOST = "8.145.51.129"

# 两台机器在内网互通时使用的地址。worker 注册给 server 时要使用内网地址，
# 不能使用 127.0.0.1，否则另一台机器访问不到。
SERVER_PRIVATE_IP = "192.168.0.136"
WORKER_PRIVATE_IP = "192.168.0.139"
SECONDARY_WORKER_PRIVATE_IP = "192.168.0.138"

# 分布式压测使用的固定隔离端口。它们必须提前确认空闲。
SERVER_PORT = 8099
WORKER_PORT = 8000
MODEL_PORT = 8888
OBS_PORT = 18002

# 这是正式运行中的 adapter-core。压测脚本可以另起隔离 server，
# 但不能误停这个 PID，也不能让它的监听端口发生变化。
PROTECTED_PID = 0
PROTECTED_PORTS = (50052, 8088, 8077)

# SSH 主机指纹用于确认“连上的确实是预期机器”。如果云主机重装或换机，
# 这里会报错，需要人工重新确认后再更新。
EXPECTED_HOST_FINGERPRINTS = {
    SERVER_HOST: "SHA256:rhrO15uNM5EoSY/4coio0s2iYkV7e+t2vaSE0G5Uqf8",
    "8.130.65.20": "SHA256:Ilv9s1diS4C4AZR0fSnxCjQs7fANg1Vyz1IqGAhTQKU",
    "8.145.51.129": "SHA256:VaErDqKKvTkmyepP+cYoQGzqDO8Chz3g3dx8Euw93uw",
}
SOURCE_REPO = ""
SERVER_BIN = ""
SOURCE_WORKER_BIN = ""
SOURCE_CODE_BIN = ""


@dataclasses.dataclass(frozen=True)
class WorkerNode:
    """一台 worker 机器的地址。

    host 是 SSH 连接用的公网地址，private_ip 是 worker 向 server 注册时
    advertise 的内网地址。两者必须成对给出，否则 worker 会注册错误的地址。
    """

    host: str
    private_ip: str


# 本次压测可用的 worker 节点列表。configure_from_args 会按 CLI 重建它。
# 默认使用两台 worker 机器；如果显式传 --worker-node，则完全以命令行为准。
DEFAULT_WORKER_NODES: tuple[WorkerNode, ...] = (
    WorkerNode(WORKER_HOST, WORKER_PRIVATE_IP),
    WorkerNode(SECONDARY_WORKER_HOST, SECONDARY_WORKER_PRIVATE_IP),
)
WORKER_NODES: list[WorkerNode] = list(DEFAULT_WORKER_NODES)


def parse_worker_nodes(values: list[str] | None) -> list[WorkerNode]:
    """解析可重复的 --worker-node HOST:PRIVATE_IP 参数。

    防御式校验：
    - 格式必须是 host:private_ip，两端都不能为空；
    - host 必须已有登记的 SSH 指纹，避免连上未确认的机器；
    - 同一 host 不允许重复出现。同一台机器上端口是共享资源，重复节点会让
      两批 worker 的端口检查互相冲突，属于配置错误而不是分摊。
    """
    nodes: list[WorkerNode] = []
    seen_hosts: set[str] = set()
    for value in values or []:
        text = str(value).strip()
        if ":" not in text:
            raise ValueError(f"--worker-node must be HOST:PRIVATE_IP, got {value!r}")
        host, private_ip = (part.strip() for part in text.split(":", 1))
        if not host or not private_ip:
            raise ValueError(f"--worker-node must be HOST:PRIVATE_IP, got {value!r}")
        if host not in EXPECTED_HOST_FINGERPRINTS:
            raise ValueError(
                f"--worker-node host {host} has no registered SSH fingerprint; "
                "confirm the machine first, then add it to EXPECTED_HOST_FINGERPRINTS"
            )
        if host in seen_hosts:
            raise ValueError(f"--worker-node host {host} is duplicated")
        seen_hosts.add(host)
        nodes.append(WorkerNode(host, private_ip))
    return nodes


def worker_assignments(count: int) -> list[list[int]]:
    """把 count 个 worker 索引 round-robin 分配到各节点。

    返回与 WORKER_NODES 等长的列表，第 j 项是落在节点 j 上的 worker 索引。
    worker i 固定落在节点 i % len(WORKER_NODES)，奇偶分摊；以后增加节点
    不需要改这里的逻辑。
    """
    if count <= 0:
        raise ValueError(f"worker count must be positive, got {count}")
    if not WORKER_NODES:
        raise ValueError("no worker nodes configured")
    groups: list[list[int]] = [[] for _ in WORKER_NODES]
    for index in range(count):
        groups[index % len(WORKER_NODES)].append(index)
    return groups


def connect_worker_nodes(password: str) -> dict[str, paramiko.SSHClient]:
    """连接全部 worker 节点，返回 host -> SSHClient 的映射。

    每个节点一条独立 SSH 连接，指纹校验与 connect() 完全一致。
    """
    return {node.host: connect(node.host, password) for node in WORKER_NODES}


def add_runtime_arguments(
    parser: argparse.ArgumentParser,
    *,
    require_code_plugin: bool,
) -> None:
    """Add explicit deployment, process-protection and port arguments."""
    parser.add_argument("--source-repo", required=True)
    parser.add_argument("--server-bin", required=True)
    parser.add_argument("--worker-bin", required=True)
    if require_code_plugin:
        parser.add_argument("--code-plugin-bin", required=True)
    else:
        parser.add_argument("--code-plugin-bin", default="")
    parser.add_argument("--protected-pid", type=int, required=True)
    parser.add_argument(
        "--protected-port",
        type=int,
        action="append",
        help="Production adapter listener to lock. Repeat for each port; defaults to 50052/8077/8088.",
    )
    parser.add_argument("--server-host", default=SERVER_HOST)
    parser.add_argument("--worker-host", default=WORKER_HOST)
    parser.add_argument("--server-private-ip", default=SERVER_PRIVATE_IP)
    parser.add_argument("--worker-private-ip", default=WORKER_PRIVATE_IP)
    parser.add_argument(
        "--worker-node",
        action="append",
        default=[],
        metavar="HOST:PRIVATE_IP",
        help=(
            "Worker node as SSH host plus private IP. Repeat for each node; "
            "when present it overrides --worker-host/--worker-private-ip."
        ),
    )
    parser.add_argument("--server-port", type=int, default=8099)
    parser.add_argument("--worker-port", type=int, default=8000)
    parser.add_argument("--model-port", type=int, default=8888)
    parser.add_argument("--obs-port", type=int, default=18002)


def configure_from_args(args: argparse.Namespace) -> None:
    """Apply CLI values once before any SSH connection or process action."""
    global SERVER_HOST, WORKER_HOST, SERVER_PRIVATE_IP, WORKER_PRIVATE_IP
    global SERVER_PORT, WORKER_PORT, MODEL_PORT, OBS_PORT
    global PROTECTED_PID, PROTECTED_PORTS, SOURCE_REPO
    global SERVER_BIN, SOURCE_WORKER_BIN, SOURCE_CODE_BIN
    global WORKER_NODES

    if args.protected_pid <= 1:
        raise ValueError("--protected-pid must be greater than 1")
    for name in ("source_repo", "server_bin", "worker_bin"):
        value = str(getattr(args, name, "")).strip()
        if not value.startswith("/"):
            raise ValueError(f"--{name.replace('_', '-')} must be an absolute path")
    code_bin = str(getattr(args, "code_plugin_bin", "")).strip()
    if code_bin and not code_bin.startswith("/"):
        raise ValueError("--code-plugin-bin must be an absolute path")

    SERVER_HOST = args.server_host
    SERVER_PRIVATE_IP = args.server_private_ip
    # --worker-node 可重复传入多台的 host:private_ip，传入时以它为准；
    # 未传入且使用默认 worker-host/private-ip 时，使用两台默认 worker。
    # 如果显式改了 --worker-host/--worker-private-ip，则保留单节点兼容行为。
    # WORKER_HOST/WORKER_PRIVATE_IP 仍指向第一个节点，保证只读兼容层的旧代码
    # （包括 suite preflight 和既有测试）行为不变。
    explicit_nodes = parse_worker_nodes(getattr(args, "worker_node", None))
    if explicit_nodes:
        WORKER_NODES = explicit_nodes
        WORKER_HOST = explicit_nodes[0].host
        WORKER_PRIVATE_IP = explicit_nodes[0].private_ip
    elif (
        args.worker_host == DEFAULT_WORKER_NODES[0].host
        and args.worker_private_ip == DEFAULT_WORKER_NODES[0].private_ip
    ):
        WORKER_NODES = list(DEFAULT_WORKER_NODES)
        WORKER_HOST = DEFAULT_WORKER_NODES[0].host
        WORKER_PRIVATE_IP = DEFAULT_WORKER_NODES[0].private_ip
    else:
        WORKER_HOST = args.worker_host
        WORKER_PRIVATE_IP = args.worker_private_ip
        WORKER_NODES = [WorkerNode(WORKER_HOST, WORKER_PRIVATE_IP)]
    SERVER_PORT = args.server_port
    WORKER_PORT = args.worker_port
    MODEL_PORT = args.model_port
    OBS_PORT = args.obs_port
    PROTECTED_PID = args.protected_pid
    PROTECTED_PORTS = tuple(args.protected_port or (50052, 8077, 8088))
    SOURCE_REPO = args.source_repo.rstrip("/")
    SERVER_BIN = args.server_bin
    SOURCE_WORKER_BIN = args.worker_bin
    SOURCE_CODE_BIN = code_bin


def source_and_binary_manifest(
    client: paramiko.SSHClient,
    *,
    include_code_plugin: bool,
) -> dict:
    """Return the exact Git revision and hashes of binaries used by a run."""
    paths = {
        "server": SERVER_BIN,
        "worker": SOURCE_WORKER_BIN,
    }
    if include_code_plugin:
        paths["code_plugin"] = SOURCE_CODE_BIN
    required = [SOURCE_REPO, *paths.values()]
    tests = [f"test -d {q(required[0])}"] + [f"test -x {q(path)}" for path in required[1:]]
    run(client, " && ".join(tests), timeout=20)
    _, git_sha, _ = run(client, f"git -C {q(SOURCE_REPO)} rev-parse HEAD", timeout=20)
    _, git_status, _ = run(
        client,
        f"git -C {q(SOURCE_REPO)} status --porcelain --untracked-files=normal",
        timeout=20,
    )
    binaries = {}
    for name, path in paths.items():
        _, output, _ = run(client, f"sha256sum {q(path)} && stat -c %s {q(path)}", timeout=30)
        lines = output.splitlines()
        binaries[name] = {
            "path": path,
            "sha256": lines[0].split()[0],
            "size_bytes": int(lines[1]),
        }
    return {
        "source_repo": SOURCE_REPO,
        "git_sha": git_sha.strip(),
        "git_clean": not bool(git_status.strip()),
        "git_status": git_status.splitlines(),
        "binaries": binaries,
    }


def q(value: str) -> str:
    """把字符串转成安全的 shell 参数。

    远端命令里经常要拼路径，例如 /tmp/uenv-xxx/server.yaml。
    统一用 shlex.quote 可以避免空格或特殊字符破坏命令结构。
    """
    return shlex.quote(value)


def connect(host: str, password: str) -> paramiko.SSHClient:
    """连接远端机器，并校验 SSH 主机指纹。

    这里只创建 SSH 连接，不执行任何业务命令。指纹校验失败时立即断开，
    防止后续命令被发到错误机器。
    """
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(
        host,
        username="root",
        password=password,
        timeout=10,
        banner_timeout=10,
        auth_timeout=10,
    )
    transport = client.get_transport()
    if transport is None:
        client.close()
        raise RuntimeError(f"SSH transport unavailable for {host}")
    transport.set_keepalive(30)
    key = transport.get_remote_server_key()
    fingerprint = getattr(key, "fingerprint", "")
    if not fingerprint:
        digest = base64.b64encode(hashlib.sha256(key.asbytes()).digest()).decode("ascii").rstrip("=")
        fingerprint = f"SHA256:{digest}"
    expected = EXPECTED_HOST_FINGERPRINTS[host]
    if fingerprint != expected:
        client.close()
        raise RuntimeError(
            f"SSH host key mismatch for {host}: actual={fingerprint}, expected={expected}"
        )
    print(f"[ssh] host={host} key_type={key.get_name()} fingerprint_sha256={fingerprint}")
    return client


def run(
    client: paramiko.SSHClient,
    command: str,
    *,
    timeout: int = 120,
    check: bool = True,
) -> tuple[int, str, str]:
    """在远端执行一条命令，返回退出码、stdout、stderr。

    check=True 时，非 0 退出码会直接抛异常。这样调用方不用每次手写
    退出码判断，也能把失败命令和输出一起带出来，方便排查。
    """
    _, stdout, stderr = client.exec_command(command, timeout=timeout)
    channel = stdout.channel
    deadline = time.monotonic() + timeout
    stdout_chunks: list[bytes] = []
    stderr_chunks: list[bytes] = []

    # stdout.read() 后再 stderr.read() 会在大输出时死锁：远端命令先把
    # stderr 或 SSH channel 窗口写满，进而无法退出；本地则还在等待 stdout
    # EOF。两个流必须在命令运行期间交替排空。
    status: int | None = None
    while True:
        while channel.recv_ready():
            stdout_chunks.append(channel.recv(65536))
        while channel.recv_stderr_ready():
            stderr_chunks.append(channel.recv_stderr(65536))

        if status is None and channel.exit_status_ready():
            status = channel.recv_exit_status()
        # Paramiko 可能先收到退出状态、再收到 channel 里的最后一段输出；
        # 必须等到 channel 关闭且两个接收缓冲均排空后才能返回。
        if (
            status is not None
            and channel.closed
            and not channel.recv_ready()
            and not channel.recv_stderr_ready()
        ):
            break
        if time.monotonic() >= deadline:
            channel.close()
            raise TimeoutError(f"remote command timed out after {timeout}s: {command}")
        time.sleep(0.01)

    out = b"".join(stdout_chunks).decode("utf-8", errors="replace")
    err = b"".join(stderr_chunks).decode("utf-8", errors="replace")
    if check and status != 0:
        raise RuntimeError(
            f"remote command failed status={status}: {command}\nstdout={out}\nstderr={err}"
        )
    return status, out, err


def put_text(client: paramiko.SSHClient, path: str, text: str, mode: int = 0o644) -> None:
    """通过 SFTP 写入一个文本文件，并设置权限。

    分布式脚本会临时生成 server.yaml、worker.yaml、client.py 等文件，
    这些文件都通过这个函数写到远端运行目录。
    """
    with client.open_sftp() as sftp:
        with sftp.open(path, "wb") as remote:
            remote.write(text.encode())
        sftp.chmod(path, mode)


def put_texts(
    client: paramiko.SSHClient,
    documents: dict[str, tuple[str, int]],
) -> None:
    """Write many small owned files through one SFTP session.

    A 1024-Worker fleet needs one YAML per real Worker.  Opening a fresh SFTP
    session for every YAML turns startup into thousands of SSH round trips, so
    the scale path batches those writes without changing file ownership or
    permissions.
    """
    with client.open_sftp() as sftp:
        for path, (content, mode) in documents.items():
            with sftp.open(path, "wb") as remote:
                remote.write(content.encode())
            sftp.chmod(path, mode)


def get_text(client: paramiko.SSHClient, path: str) -> str:
    """通过 SFTP 读取远端文本文件。

    常用于拉取 result.json、server.log、worker.log 等压测产物。
    """
    with client.open_sftp() as sftp:
        with sftp.open(path, "rb") as remote:
            return remote.read().decode("utf-8", errors="replace")


def process_starttime(client: paramiko.SSHClient, pid: int) -> int:
    """读取 Linux /proc/<pid>/stat 里的进程启动时间。

    PID 可能被系统复用，所以只看 PID 不够安全。PID 加启动时间一起比较，
    可以更准确地判断“是不是同一个进程”。
    """
    stat_text = get_text(client, f"/proc/{pid}/stat")
    fields = stat_text[stat_text.rfind(")") + 2 :].split()
    return int(fields[19])


def process_exe(client: paramiko.SSHClient, pid: int) -> str:
    """读取某个 PID 实际执行的二进制路径。"""
    _, out, _ = run(client, f"readlink -f /proc/{pid}/exe", timeout=10)
    return out.strip()


def process_cmdline(client: paramiko.SSHClient, pid: int) -> str:
    """读取某个 PID 的完整命令行参数。"""
    with client.open_sftp() as sftp:
        with sftp.open(f"/proc/{pid}/cmdline", "rb") as remote:
            return " ".join(
                item.decode(errors="replace")
                for item in remote.read().split(b"\0")
                if item
            )


def listeners(client: paramiko.SSHClient) -> str:
    """列出远端当前 TCP 监听端口和对应进程。"""
    _, out, _ = run(client, "ss -H -lntp", timeout=15)
    return out


def assert_port_free(client: paramiko.SSHClient, port: int, host: str) -> None:
    """确认某个端口没有被占用。

    压测启动前必须先做这个检查。否则新 server/worker 可能启动失败，
    或者更严重地占用用户已经在使用的端口。
    """
    for line in listeners(client).splitlines():
        if f":{port} " in line:
            raise RuntimeError(f"{host}: port {port} is already occupied: {line}")


def assert_ports_free(client: paramiko.SSHClient, ports: list[int], host: str) -> None:
    """Check a complete port set from one listener snapshot."""
    expected = set(ports)
    occupied = []
    for line in listeners(client).splitlines():
        for port in expected:
            if f":{port} " in line:
                occupied.append({"port": port, "listener": line})
    if occupied:
        raise RuntimeError(f"{host}: requested ports are occupied: {occupied[:20]}")


def _listener_pid(lines: str, port: int) -> tuple[int, str]:
    matching = [line for line in lines.splitlines() if f":{port} " in line]
    if len(matching) != 1:
        raise RuntimeError(f"protected port {port} must have exactly one listener: {matching}")
    pids = {int(value) for value in re.findall(r"pid=(\d+)", matching[0])}
    if len(pids) != 1:
        raise RuntimeError(f"protected port {port} listener PID is ambiguous: {matching[0]}")
    return pids.pop(), matching[0]


def protected_snapshot(client: paramiko.SSHClient) -> dict:
    """记录正式 adapter-core 的身份和端口状态。

    这个快照包含 PID、可执行文件、命令行、启动时间和监听端口。
    压测前后各检查一次，用来证明压测没有影响正式 server。

    注意：快照以"当前实际监听者"为准，不再硬性要求等于 PROTECTED_PID——
    生产进程被外部重启（systemd restart）是允许的，身份一致性由
    assert_protected_unchanged 按 exe/cmdline/ports 判断。
    重启瞬间端口可能短暂无监听，因此对监听采集做有限重试。
    """
    last_error: Exception | None = None
    for _attempt in range(5):
        try:
            current_listeners = listeners(client)
            listener_records = {}
            observed_pids = set()
            for port in PROTECTED_PORTS:
                observed_pid, line = _listener_pid(current_listeners, port)
                observed_pids.add(observed_pid)
                listener_records[str(port)] = line
            if len(observed_pids) != 1:
                raise RuntimeError(
                    f"protected listeners must be owned by exactly one PID: "
                    f"observed={sorted(observed_pids)}"
                )
            observed = observed_pids.pop()
            exe = process_exe(client, observed)
            return {
                "pid": observed,
                "exe": exe,
                "cmdline": process_cmdline(client, observed),
                "starttime_ticks": process_starttime(client, observed),
                "ports": list(PROTECTED_PORTS),
                "listener_records": listener_records,
            }
        except RuntimeError as exc:
            last_error = exc
            time.sleep(2)
    raise RuntimeError(f"protected snapshot failed after retries: {last_error}")


def assert_protected_unchanged(client: paramiko.SSHClient, before: dict) -> None:
    """确认正式 server 身份未被压测破坏。

    判定口径是"服务身份"而不是"进程号"：exe / cmdline / 监听端口集合不变，
    即视为未被破坏。生产进程被外部重启（pid/starttime 变化）不算违规——
    此时同步更新 PROTECTED_PID 跟踪新进程，避免后续清理误杀，
    并打印事件方便审计。exe/cmdline/端口任一变化仍然 fail closed。
    """
    global PROTECTED_PID
    after = protected_snapshot(client)
    if after == before:
        return
    same_identity = (
        after.get("exe") == before.get("exe")
        and after.get("cmdline") == before.get("cmdline")
        and sorted(after.get("ports", [])) == sorted(before.get("ports", []))
    )
    if same_identity and after.get("pid") != before.get("pid"):
        PROTECTED_PID = int(after["pid"])
        print(
            "[protected] production server restarted externally: "
            f"pid {before.get('pid')} -> {after['pid']}; "
            "identity (exe/cmdline/ports) unchanged, continuing",
            flush=True,
        )
        return
    raise RuntimeError(
        "protected server changed: "
        f"before={json.dumps(before, sort_keys=True)}, "
        f"after={json.dumps(after, sort_keys=True)}"
    )


def start_owned(
    client: paramiko.SSHClient,
    command: str,
    log_path: str,
    expected_exe: str,
    expected_cmdline: str,
) -> int:
    """启动一个本次压测拥有的远端进程。

    setsid 会让子进程进入新的进程组。后续清理时可以按进程组发送信号，
    把这个进程创建的子进程一起停掉。

    启动后会检查实际 exe 和命令行片段，确认 PID 确实属于本次压测。
    如果身份不匹配，脚本会拒绝继续，避免误清理其它进程。
    """
    start = (
        f"nohup setsid {command} >{q(log_path)} 2>&1 </dev/null & "
        "pid=$!; echo $pid"
    )
    _, out, _ = run(client, start, timeout=15)
    pid = int(out.strip().splitlines()[-1])
    if pid in {1, PROTECTED_PID}:
        raise RuntimeError(f"refusing unsafe owned PID {pid}")
    time.sleep(1)
    exe = process_exe(client, pid)
    cmdline = process_cmdline(client, pid)
    if exe != expected_exe or expected_cmdline not in cmdline:
        raise RuntimeError(
            f"owned PID identity mismatch pid={pid} exe={exe!r} cmdline={cmdline!r}"
        )
    print(f"[start] pid={pid} exe={exe} log={log_path}")
    return pid


def stop_owned(
    client: paramiko.SSHClient,
    pid: int | None,
    expected_exe: str,
    expected_cmdline: str,
) -> None:
    """停止一个本次压测拥有的远端进程。

    清理前再次检查 exe 和命令行，确认目标进程身份正确。先发 SIGTERM，
    等待一小段时间；如果仍未退出，再发 SIGKILL。整个函数只清理本次
    压测明确记录的 PID，不会按进程名批量杀进程。
    """
    if not pid:
        return
    if pid in {1, PROTECTED_PID}:
        raise RuntimeError(f"refusing to stop unsafe PID {pid}")
    status, _, _ = run(client, f"test -r /proc/{pid}/status", timeout=10, check=False)
    if status != 0:
        return
    exe = process_exe(client, pid)
    cmdline = process_cmdline(client, pid)
    if exe != expected_exe or expected_cmdline not in cmdline:
        raise RuntimeError(
            f"refusing cleanup identity mismatch pid={pid} exe={exe!r} cmdline={cmdline!r}"
        )
    run(client, f"kill -TERM -- -{pid}", timeout=10, check=False)
    for _ in range(20):
        status, _, _ = run(client, f"test -r /proc/{pid}/status", timeout=5, check=False)
        if status != 0:
            print(f"[cleanup] pid={pid} exited after SIGTERM")
            return
        time.sleep(0.25)
    run(client, f"kill -KILL -- -{pid}", timeout=10, check=False)
    time.sleep(0.5)
    status, _, _ = run(client, f"test -r /proc/{pid}/status", timeout=5, check=False)
    if status == 0:
        raise RuntimeError(f"owned PID {pid} survived SIGKILL")
    print(f"[cleanup] pid={pid} required SIGKILL")
