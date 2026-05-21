param(
    [Parameter(Mandatory = $true)]
    [string]$HostIp,

    [int]$Port = 9222,

    [ValidateSet("auto", "edge", "chrome")]
    [string]$Browser = "auto",

    [string]$ProfileDir = "$env:TEMP\ccbg-cdp"
)

$ErrorActionPreference = "Stop"
$LocalUrl = "http://127.0.0.1:$Port/json/version"
$LanUrl = "http://$HostIp:$Port/json/version"

if ($HostIp -match '^(127\.|localhost$)') {
    throw "HostIp must be the browser host LAN IP, not localhost or 127.0.0.1."
}

function Test-IsAdministrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-CdpUrl {
    param([string]$Url)
    try {
        Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 3 | Out-Null
        return $true
    }
    catch {
        return $false
    }
}

function Wait-CdpUrl {
    param(
        [string]$Url,
        [string]$Label
    )
    for ($attempt = 0; $attempt -lt 20; $attempt++) {
        if (Test-CdpUrl -Url $Url) {
            return
        }
        Start-Sleep -Seconds 1
    }
    throw "$Label did not become reachable: $Url"
}

function Resolve-BrowserExecutable {
    $candidates = switch ($Browser) {
        "edge"   { @("msedge.exe") }
        "chrome" { @("chrome.exe") }
        default  { @("msedge.exe", "chrome.exe") }
    }

    foreach ($candidate in $candidates) {
        $command = Get-Command $candidate -ErrorAction SilentlyContinue
        if ($command) {
            return $command.Source
        }
    }

    $pathCandidates = switch ($Browser) {
        "edge" {
            @(
                "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
                "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
            )
        }
        "chrome" {
            @(
                "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
                "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe"
            )
        }
        default {
            @(
                "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe",
                "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
                "$env:ProgramFiles\Google\Chrome\Application\chrome.exe",
                "${env:ProgramFiles(x86)}\Google\Chrome\Application\chrome.exe"
            )
        }
    }

    foreach ($candidate in $pathCandidates) {
        if ($candidate -and (Test-Path $candidate)) {
            return $candidate
        }
    }

    throw "No usable browser executable found for Browser=$Browser."
}

if (-not (Test-IsAdministrator)) {
    throw "Run this PowerShell script as Administrator so it can manage firewall and portproxy rules."
}

Write-Host "1) Ensuring the Windows firewall allows TCP $Port"
$ruleName = "CCBG CDP $Port"
if (-not (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Action Allow -Protocol TCP -LocalPort $Port | Out-Null
}

Write-Host "2) Ensuring IP Helper is running for netsh portproxy"
$service = Get-Service iphlpsvc -ErrorAction SilentlyContinue
if ($service -and $service.Status -ne "Running") {
    Start-Service iphlpsvc
}

Write-Host "3) Ensuring loopback CDP is up on $LocalUrl"
if (-not (Test-CdpUrl -Url $LocalUrl)) {
    $browserExecutable = Resolve-BrowserExecutable
    New-Item -ItemType Directory -Path $ProfileDir -Force | Out-Null
    Start-Process -FilePath $browserExecutable -ArgumentList @(
        "--remote-debugging-port=$Port",
        "--user-data-dir=$ProfileDir",
        "--no-first-run",
        "--no-default-browser-check"
    ) | Out-Null
    Wait-CdpUrl -Url $LocalUrl -Label "Loopback CDP"
}
else {
    Write-Host "Loopback CDP already reachable on $LocalUrl"
}

Write-Host "4) Recreating the LAN bridge $HostIp`:$Port -> 127.0.0.1`:$Port"
netsh interface portproxy delete v4tov4 listenaddress=$HostIp listenport=$Port | Out-Null
netsh interface portproxy add v4tov4 listenaddress=$HostIp listenport=$Port connectaddress=127.0.0.1 connectport=$Port | Out-Null

Write-Host "5) Verifying both endpoints"
Wait-CdpUrl -Url $LocalUrl -Label "Loopback CDP"
Wait-CdpUrl -Url $LanUrl -Label "LAN CDP bridge"

Write-Host ""
Write-Host "CDP browser host is ready."
Write-Host ""
Write-Host "Loopback:"
Write-Host "  $LocalUrl"
Write-Host ""
Write-Host "LAN:"
Write-Host "  $LanUrl"
Write-Host ""
Write-Host "Use this LAN URL in carrier-cloud-blob-gateway Admin -> Browser / CDP."
Write-Host "Do not enter localhost or 127.0.0.1 there."
