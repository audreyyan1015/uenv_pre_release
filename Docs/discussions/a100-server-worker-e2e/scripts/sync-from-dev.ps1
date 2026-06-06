# 从 Windows 开发机 rsync/scp 同步 UEnv 到 A100（需 OpenSSH scp）
# 用法: .\sync-from-dev.ps1 [-Target A|B|Both]

param(
    [ValidateSet("A", "B", "Both")]
    [string]$Target = "Both"
)

$ErrorActionPreference = "Stop"
$RepoRoot = "d:\code\UEnv"
$RemoteRoot = "/root/UEnv"
$HostIP = "219.147.100.43"
$Secrets = Join-Path $RepoRoot "secrets"

$Machines = @{
    A = @{ Port = 7143; Key = "9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143" }
    B = @{ Port = 7142; Key = "2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142" }
}

function Sync-ToMachine($Name) {
    $m = $Machines[$Name]
    $keyPath = Join-Path $Secrets $m.Key
    $sshTarget = "root@${HostIP}"
    $sshArgs = @("-i", $keyPath, "-p", $m.Port, "-o", "StrictHostKeyChecking=no")

    Write-Host "`n>>> 同步到机器 $Name ($HostIP`:$($m.Port)) -> $RemoteRoot"

    $initScript = Join-Path $RepoRoot "Docs\discussions\a100-server-worker-e2e\scripts\init-e2e-layout.sh"
    scp @sshArgs $initScript "${sshTarget}:/tmp/init-e2e-layout.sh"
    ssh @sshArgs $sshTarget "bash /tmp/init-e2e-layout.sh"

    # 排除 target/.git 等大目录；优先 scp 递归（Windows 自带 OpenSSH）
    $tarExclude = @(
        "--exclude=target",
        "--exclude=.git",
        "--exclude=secrets",
        "--exclude=node_modules"
    )

    # 若 WSL tar 可用则更快；否则 fallback scp
    $wslTar = Get-Command wsl -ErrorAction SilentlyContinue
    if ($wslTar) {
        $excludeArgs = ($tarExclude | ForEach-Object { $_ }) -join " "
        wsl bash -lc "cd '$($RepoRoot -replace '\\','/')' && tar czf - $excludeArgs ." |
            ssh @sshArgs $sshTarget "tar xzf - -C $RemoteRoot"
    } else {
        Write-Warning "未检测到 WSL，使用 scp（较慢）。建议安装 WSL 或使用 Git Bash rsync。"
        scp @sshArgs -r "$RepoRoot\*" "${sshTarget}:${RemoteRoot}/"
    }

    Write-Host ">>> 机器 $Name 同步完成"
}

$targets = if ($Target -eq "Both") { @("A", "B") } else { @($Target) }
foreach ($t in $targets) { Sync-ToMachine $t }

Write-Host "`n完成。请在各机器执行 remote-build.sh（见 scripts/remote-build.sh）"
