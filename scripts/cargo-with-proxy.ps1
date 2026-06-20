# SPDX-License-Identifier: LicenseRef-CCBG-Commercial
# Copyright (c) 2026 walky

$ErrorActionPreference = "Stop"
$CargoArgs = $args

if (-not $CargoArgs -or $CargoArgs.Count -eq 0) {
    throw "Provide cargo arguments, for example: .\scripts\cargo-with-proxy.ps1 test -p mcp-server"
}

$ProxyUrl = if (-not [string]::IsNullOrWhiteSpace($env:CCBG_CARGO_PROXY_URL)) {
    $env:CCBG_CARGO_PROXY_URL
}
else {
    "socks5h://127.0.0.1:10808"
}

$NoProxy = if (-not [string]::IsNullOrWhiteSpace($env:CCBG_CARGO_NO_PROXY)) {
    $env:CCBG_CARGO_NO_PROXY
}
else {
    "127.0.0.1,localhost,::1"
}

$CargoHttpTimeoutSeconds = if (-not [string]::IsNullOrWhiteSpace($env:CCBG_CARGO_HTTP_TIMEOUT)) {
    $env:CCBG_CARGO_HTTP_TIMEOUT
}
else {
    "120"
}

$savedEnv = @{
    HTTP_PROXY = $env:HTTP_PROXY
    HTTPS_PROXY = $env:HTTPS_PROXY
    ALL_PROXY = $env:ALL_PROXY
    NO_PROXY = $env:NO_PROXY
    CARGO_HTTP_TIMEOUT = $env:CARGO_HTTP_TIMEOUT
}

try {
    $env:HTTP_PROXY = $ProxyUrl
    $env:HTTPS_PROXY = $ProxyUrl
    $env:ALL_PROXY = $ProxyUrl
    $env:NO_PROXY = $NoProxy
    $env:CARGO_HTTP_TIMEOUT = $CargoHttpTimeoutSeconds.ToString()

    Write-Host "Using cargo proxy: $ProxyUrl"
    Write-Host "Using NO_PROXY: $NoProxy"
    & cargo @CargoArgs
    exit $LASTEXITCODE
}
finally {
    foreach ($entry in $savedEnv.GetEnumerator()) {
        if ($null -eq $entry.Value) {
            Remove-Item "Env:$($entry.Key)" -ErrorAction SilentlyContinue
        }
        else {
            Set-Item "Env:$($entry.Key)" $entry.Value
        }
    }
}
