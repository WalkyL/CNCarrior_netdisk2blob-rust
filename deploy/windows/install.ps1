# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky
[CmdletBinding()]
param(
  [string]$InstallDir = "$env:LOCALAPPDATA\AGI2030\ccbg",
  [string]$TaskName = "CCBG-GatewayD"
)

$ErrorActionPreference = "Stop"
$PackageRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Gatewayd = Join-Path $PackageRoot "bin\gatewayd.exe"
if (!(Test-Path $Gatewayd)) {
  throw "missing $Gatewayd"
}

$DataDir = Join-Path $env:LOCALAPPDATA "AGI2030\ccbg-data"
$ConfigDir = Join-Path $InstallDir "config"
$LogDir = Join-Path $InstallDir "logs"
$AssetDir = Join-Path $InstallDir "assets\admin"
New-Item -ItemType Directory -Force -Path $InstallDir,$DataDir,$ConfigDir,$LogDir,$AssetDir | Out-Null

Copy-Item -Force $Gatewayd (Join-Path $InstallDir "gatewayd.exe")
Copy-Item -Force (Join-Path $PackageRoot "assets\admin\index.html") (Join-Path $AssetDir "index.html")
Copy-Item -Recurse -Force (Join-Path $PackageRoot "config\*") $ConfigDir

$Runner = Join-Path $InstallDir "run-gatewayd.ps1"
@"
`$env:CCBG_ADMIN_ASSET_PATH = "$AssetDir\index.html"
`$env:CCBG_CONFIG_DIR = "$ConfigDir"
`$env:CCBG_DATA_DIR = "$DataDir"
Set-Location "$DataDir"
& "$InstallDir\gatewayd.exe" *> "$LogDir\gatewayd.log"
"@ | Set-Content -Encoding UTF8 -Path $Runner

$Action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$Runner`""
$Trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$Principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel LeastPrivilege
$Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1)
Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Principal $Principal -Settings $Settings -Force | Out-Null
Start-ScheduledTask -TaskName $TaskName

Write-Host "installed $TaskName"
Write-Host "health: Invoke-WebRequest http://127.0.0.1:61080/healthz"
