# DeepSeek Price Clock for Windows 11 - a tray icon, no window, no installer,
# no dependencies. Runs on the PowerShell 5.1 that ships with Windows.
#
# Run it:   powershell -NoProfile -ExecutionPolicy Bypass -File .\DeepSeekPriceClock.ps1
# Install:  powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
#
# Price rules ported from deepakness's extension
# https://github.com/deepakness/deepseek-price-clock (MIT (c) 2026 deepakness).

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

. (Join-Path $PSScriptRoot 'PriceClock.ps1')

# Captured here because event handlers do not always see $PSCommandPath.
$script:AppPath = $PSCommandPath

$PricingUrl = 'https://api-docs.deepseek.com/quick_start/pricing'
$OffColor = [System.Drawing.Color]::FromArgb(87, 227, 137)
$PeakColor = [System.Drawing.Color]::FromArgb(255, 123, 114)

# Only one clock per session, otherwise two timers fight over the same tray.
$mutex = New-Object System.Threading.Mutex($false, 'Local\DeepSeekPriceClock')
if (-not $mutex.WaitOne(0)) {
    exit
}

# A tray dot drawn in code, so the project ships no binary assets. Built once
# for each state: two icon handles for the life of the process, no leak.
function New-DotIcon {
    param([System.Drawing.Color]$Color)

    $size = 32
    $bitmap = New-Object System.Drawing.Bitmap $size, $size
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.Clear([System.Drawing.Color]::Transparent)

    $halo = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(70, $Color))
    $dot = New-Object System.Drawing.SolidBrush $Color
    try {
        $graphics.FillEllipse($halo, 2, 2, 28, 28)
        $graphics.FillEllipse($dot, 8, 8, 16, 16)
    }
    finally {
        $halo.Dispose()
        $dot.Dispose()
        $graphics.Dispose()
    }

    $handle = $bitmap.GetHicon()
    $icon = [System.Drawing.Icon]::FromHandle($handle)
    $bitmap.Dispose()
    $icon
}

$script:OffIcon = New-DotIcon $OffColor
$script:PeakIcon = New-DotIcon $PeakColor

# --- Context menu -----------------------------------------------------------

$menu = New-Object System.Windows.Forms.ContextMenuStrip
$menu.ShowImageMargin = $false

function New-InfoItem {
    param([string]$Text, [System.Windows.Forms.ToolStripItem]$Parent, [switch]$Bold)
    $item = New-Object System.Windows.Forms.ToolStripMenuItem $Text
    $item.Enabled = $false
    if ($Bold) {
        $item.Font = New-Object System.Drawing.Font($item.Font, [System.Drawing.FontStyle]::Bold)
    }
    $Parent.DropDownItems.Add($item) | Out-Null
    $item
}

$headerItem = New-Object System.Windows.Forms.ToolStripMenuItem 'Off-peak'
$headerItem.Enabled = $false
$headerItem.Font = New-Object System.Drawing.Font($headerItem.Font, [System.Drawing.FontStyle]::Bold)
$menu.Items.Add($headerItem) | Out-Null

$countdownItem = New-Object System.Windows.Forms.ToolStripMenuItem 'Working out the schedule'
$countdownItem.Enabled = $false
$menu.Items.Add($countdownItem) | Out-Null

$menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator)) | Out-Null

$switchesMenu = New-Object System.Windows.Forms.ToolStripMenuItem 'Next switches'
$script:SwitchItems = New-Object 'System.Collections.Generic.List[object]'
for ($i = 0; $i -lt 4; $i++) {
    $script:SwitchItems.Add((New-InfoItem -Text ' ' -Parent $switchesMenu))
}
$menu.Items.Add($switchesMenu) | Out-Null

$ratesMenu = New-Object System.Windows.Forms.ToolStripMenuItem 'Rates per 1M tokens'
New-InfoItem -Text 'model, input / cached input / output' -Parent $ratesMenu | Out-Null
foreach ($model in $script:Models) {
    New-InfoItem -Text $model.Name -Parent $ratesMenu -Bold | Out-Null
    New-InfoItem -Text ('  off   {0} / {1} / {2}' -f $model.Off[0], $model.Off[1], $model.Off[2]) -Parent $ratesMenu | Out-Null
    New-InfoItem -Text ('  peak  {0} / {1} / {2}' -f $model.Peak[0], $model.Peak[1], $model.Peak[2]) -Parent $ratesMenu | Out-Null
}
$menu.Items.Add($ratesMenu) | Out-Null

$menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator)) | Out-Null

$pricingItem = New-Object System.Windows.Forms.ToolStripMenuItem 'Open pricing page'
$pricingItem.add_Click({ Start-Process $PricingUrl })
$menu.Items.Add($pricingItem) | Out-Null

$autostartItem = New-Object System.Windows.Forms.ToolStripMenuItem 'Start with Windows'
$autostartItem.Checked = Test-Autostart
$autostartItem.add_Click({
    if (Test-Autostart) {
        Remove-Autostart
        $autostartItem.Checked = $false
    }
    else {
        Set-Autostart -ScriptPath $script:AppPath
        $autostartItem.Checked = $true
    }
})
$menu.Items.Add($autostartItem) | Out-Null

$script:Notify = $true
$notifyItem = New-Object System.Windows.Forms.ToolStripMenuItem 'Notify when prices switch'
$notifyItem.Checked = $true
$notifyItem.add_Click({
    $script:Notify = -not $script:Notify
    $notifyItem.Checked = $script:Notify
})
$menu.Items.Add($notifyItem) | Out-Null

$menu.Items.Add((New-Object System.Windows.Forms.ToolStripSeparator)) | Out-Null

$exitItem = New-Object System.Windows.Forms.ToolStripMenuItem 'Exit'
$menu.Items.Add($exitItem) | Out-Null

# --- Tray icon --------------------------------------------------------------

$tray = New-Object System.Windows.Forms.NotifyIcon
$tray.Icon = $script:OffIcon
$tray.Text = 'DeepSeek price clock'
$tray.ContextMenuStrip = $menu
$tray.Visible = $true

$tray.add_MouseClick({
    param($sender, $eventArgs)
    if ($eventArgs.Button -eq [System.Windows.Forms.MouseButtons]::Left) {
        $menu.Show([System.Windows.Forms.Cursor]::Position)
    }
})

$exitItem.add_Click({
    $script:Timer.Stop()
    $tray.Visible = $false
    $tray.Dispose()
    [System.Windows.Forms.Application]::ExitThread()
})

# --- Refresh ----------------------------------------------------------------

$script:LastKind = $null

function Update-Clock {
    $now = [DateTimeOffset]::UtcNow
    $kind = Get-PriceKind $now
    $status = Get-StatusText $kind
    $next = Get-NextSwitch $now
    $left = Format-Countdown ($next.Time - $now)

    $tray.Icon = if ($kind -eq 'peak') { $script:PeakIcon } else { $script:OffIcon }
    $headerItem.Text = $status
    $countdownItem.Text = '{0} until {1}, {2} from now' -f $status, $next.Time.ToLocalTime().ToString('HH:mm'), $left

    $text = 'DeepSeek {0}, {1} left' -f $status.ToLowerInvariant(), $left
    if ($text.Length -gt 63) {
        $text = $text.Substring(0, 60) + '...'
    }
    $tray.Text = $text

    $switches = @(Get-NextSwitches -Time $now -Count 4)
    for ($i = 0; $i -lt $script:SwitchItems.Count; $i++) {
        $switch = $switches[$i]
        $script:SwitchItems[$i].Text = '{0} {1}, in {2}' -f `
            $switch.Time.ToLocalTime().ToString('HH:mm'),
            (Get-StatusText $switch.Kind).ToLowerInvariant(),
            (Format-Countdown ($switch.Time - $now))
    }

    if ($null -ne $script:LastKind -and $script:LastKind -ne $kind -and $script:Notify) {
        $flash = $script:Models[0]
        if ($kind -eq 'peak') {
            $title = 'Peak pricing started'
            $body = 'DeepSeek V4.1 Flash is now {0} in, {1} out per 1M tokens.' -f $flash.Peak[0], $flash.Peak[2]
        }
        else {
            $title = 'Off-peak pricing started'
            $body = 'DeepSeek V4.1 Flash is now {0} in, {1} out per 1M tokens.' -f $flash.Off[0], $flash.Off[2]
        }
        $tray.ShowBalloonTip(8000, $title, $body, [System.Windows.Forms.ToolTipIcon]::Info)
    }
    $script:LastKind = $kind
}

$script:Timer = New-Object System.Windows.Forms.Timer
$script:Timer.Interval = 20000
$script:Timer.add_Tick({ Update-Clock })
$script:Timer.Start()

Update-Clock
[System.Windows.Forms.Application]::Run((New-Object System.Windows.Forms.ApplicationContext))
