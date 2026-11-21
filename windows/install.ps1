[CmdletBinding()]
param(
    [switch]$StartNow,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'PriceClock.ps1')

if ($Uninstall) {
    Remove-Autostart
    Write-Host 'Removed the autostart entry. Use Exit in the tray menu to stop the clock.'
    return
}

$app = Join-Path $PSScriptRoot 'DeepSeekPriceClock.ps1'
if (-not (Test-Path -LiteralPath $app)) {
    throw "DeepSeekPriceClock.ps1 was not found next to this script."
}

Set-Autostart -ScriptPath $app

Write-Host 'DeepSeek Price Clock will now start with Windows.'
Write-Host ''
Write-Host 'Start it now with:'
Write-Host ('  powershell -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "{0}"' -f $app)

if ($StartNow) {
    Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList @(
        '-NoProfile', '-WindowStyle', 'Hidden', '-ExecutionPolicy', 'Bypass', '-File', ('"{0}"' -f $app)
    )
    Write-Host ''
    Write-Host 'Started. Look for the dot in the notification area.'
}
