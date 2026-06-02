# A100 双机 SSH 快捷连接（Windows PowerShell）
# 用法: .\ssh-connect.ps1 A | B
# 或在 Cursor 终端中分别执行下方 ssh 命令

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..\..\..")
$Secrets = Join-Path $RepoRoot "secrets"

$Keys = @{
    A = @{
        Port = 7143
        Key  = Join-Path $Secrets "9aa460dab6678381f86a1022b8a54c9f_32e42d1c7902ce68ba6719d551645e02_8.143"
        Role = "uenv-server (ControlPlane + UEnvService)"
        IP   = "10.10.20.143"
    }
    B = @{
        Port = 7142
        Key  = Join-Path $Secrets "2a9f778a35e7d08c738c79493ba643ef_65c3b455afbe3c81a8a757c01b0faae8_8.142"
        Role = "uenv-worker (gsm8k)"
        IP   = "10.10.20.142"
    }
}

$HostIP = "219.147.100.43"
$Target = $args[0]
if (-not $Target -or -not $Keys.ContainsKey($Target.ToUpper())) {
    Write-Host "用法: .\ssh-connect.ps1 A|B"
    Write-Host ""
    Write-Host "机器 A (7143): Server  $HostIP:7143  内网 $($Keys.A.IP)"
    Write-Host "机器 B (7142): Worker  $HostIP:7142  内网 $($Keys.B.IP)"
    exit 1
}

$cfg = $Keys[$Target.ToUpper()]
Write-Host "连接机器 $Target — $($cfg.Role)"
ssh -i $cfg.Key -p $cfg.Port "root@${HostIP}"
