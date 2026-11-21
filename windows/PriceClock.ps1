# DeepSeek peak/off-peak price logic, ported from deepakness's browser extension
# https://github.com/deepakness/deepseek-price-clock (price.js, MIT (c) 2026
# deepakness). The rules are his work, not mine. Keep SwitchMinutes,
# WeekendRuleStart and ChinaHolidays in sync with upstream.
#
# Every function takes and returns [DateTimeOffset] in UTC, so the result does
# not depend on the machine's time zone or on daylight saving changes.

$script:SwitchMinutes = @(60, 240, 360, 600)

# Since 16:00 UTC on Saturday 2026-08-22 the split only applies Monday to
# Friday Beijing time.
$script:WeekendRuleStart = [DateTimeOffset]::new(2026, 8, 22, 16, 0, 0, [TimeSpan]::Zero)
$script:BeijingOffset = [TimeSpan]::FromHours(8)

# Chinese public holidays from the State Council's yearly arrangement, as
# Beijing dates. DeepSeek bills them off-peak all day.
$script:ChinaHolidays = @(
    @('2026-09-25', '2026-09-27'), # Mid-Autumn
    @('2026-10-01', '2026-10-07'), # National Day
    # 2027
    @('2027-01-01', '2027-01-03'), # New Year
    @('2027-02-05', '2027-02-13'), # Spring Festival
    @('2027-04-03', '2027-04-05'), # Qingming
    @('2027-05-01', '2027-05-05'), # Labour Day
    @('2027-06-09', '2027-06-09'), # Dragon Boat
    @('2027-09-15', '2027-09-15'), # Mid-Autumn
    @('2027-10-01', '2027-10-07')  # National Day
)

# Rates per 1M tokens (input cache hit, input cache miss, output).
$script:Models = @(
    [pscustomobject]@{
        Name = 'V4.1 Flash'
        Off  = @('$0.15', '$0.003', '$0.60')
        Peak = @('$0.30', '$0.006', '$1.20')
    },
    [pscustomobject]@{
        Name = 'V4 Pro'
        Off  = @('$0.66', '$0.022', '$1.98')
        Peak = @('$1.32', '$0.044', '$3.96')
    }
)

function Get-BeijingTime {
    param([DateTimeOffset]$Time)
    $Time.ToOffset($script:BeijingOffset)
}

function Get-BeijingDateKey {
    param([DateTimeOffset]$Time)
    (Get-BeijingTime $Time).ToString('yyyy-MM-dd', [System.Globalization.CultureInfo]::InvariantCulture)
}

function Test-BeijingWeekend {
    param([DateTimeOffset]$Time)
    $day = (Get-BeijingTime $Time).DayOfWeek
    $day -eq [DayOfWeek]::Saturday -or $day -eq [DayOfWeek]::Sunday
}

function Test-BeijingHoliday {
    param([DateTimeOffset]$Time)
    $key = Get-BeijingDateKey $Time
    foreach ($range in $script:ChinaHolidays) {
        if ([string]::CompareOrdinal($key, $range[0]) -ge 0 -and
            [string]::CompareOrdinal($key, $range[1]) -le 0) {
            return $true
        }
    }
    $false
}

# Weekends and public holidays are off-peak all day, but only under the rule
# that started on 2026-08-22.
function Test-OffPeakAllDay {
    param([DateTimeOffset]$Time)
    if ($Time -lt $script:WeekendRuleStart) {
        return $false
    }
    (Test-BeijingWeekend $Time) -or (Test-BeijingHoliday $Time)
}

function Test-SwitchInstant {
    param([DateTimeOffset]$Time)
    $minuteOfDay = $Time.UtcDateTime.Hour * 60 + $Time.UtcDateTime.Minute
    if ($script:SwitchMinutes -notcontains $minuteOfDay) {
        return $false
    }
    -not (Test-OffPeakAllDay $Time)
}

# Which half of the pricing an instant falls in: 'peak' or 'off'.
function Get-PriceKind {
    param([DateTimeOffset]$Time)

    if (Test-OffPeakAllDay $Time) {
        return 'off'
    }

    $minuteOfDay = $Time.UtcDateTime.Hour * 60 + $Time.UtcDateTime.Minute
    $isPeak = ($minuteOfDay -ge 60 -and $minuteOfDay -lt 240) -or
              ($minuteOfDay -ge 360 -and $minuteOfDay -lt 600)

    if ($isPeak) { 'peak' } else { 'off' }
}

function Get-UtcDayStart {
    param([DateTimeOffset]$Time)
    $ticks = $Time.UtcTicks - ($Time.UtcTicks % [TimeSpan]::TicksPerDay)
    [DateTimeOffset]::new($ticks, [TimeSpan]::Zero)
}

function Get-SwitchInstantAt {
    param([DateTimeOffset]$DayStart, [int]$Day, [int]$Index)
    $offset = $Day * [TimeSpan]::TicksPerDay + $script:SwitchMinutes[$Index] * [TimeSpan]::TicksPerMinute
    $DayStart.AddTicks($offset)
}

# The next N price changes as absolute instants.
function Get-NextSwitches {
    param([DateTimeOffset]$Time, [int]$Count = 1)

    $dayStart = Get-UtcDayStart $Time
    $out = New-Object 'System.Collections.Generic.List[object]'
    $day = 0
    $index = 0

    while ($out.Count -lt $Count) {
        $instant = Get-SwitchInstantAt -DayStart $dayStart -Day $day -Index $index
        if ($instant -gt $Time -and (Test-SwitchInstant $instant)) {
            $out.Add([pscustomobject]@{
                Time = $instant
                Kind = Get-PriceKind $instant
            })
        }
        $index++
        if ($index -ge $script:SwitchMinutes.Count) {
            $index = 0
            $day++
        }
    }

    $out.ToArray()
}

function Get-NextSwitch {
    param([DateTimeOffset]$Time)
    (Get-NextSwitches -Time $Time -Count 1)[0]
}

# The instant the window you are in began. Walks back up to 21 days to cross
# any weekend plus holiday off-peak stretch.
function Get-CurrentWindowStart {
    param([DateTimeOffset]$Time)

    $dayStart = Get-UtcDayStart $Time
    $best = $null

    for ($day = -21; $day -le 0; $day++) {
        for ($index = 0; $index -lt $script:SwitchMinutes.Count; $index++) {
            $instant = Get-SwitchInstantAt -DayStart $dayStart -Day $day -Index $index
            if ($instant -le $Time -and (Test-SwitchInstant $instant)) {
                if ($null -eq $best -or $instant -gt $best) {
                    $best = $instant
                }
            }
        }
    }

    if ($null -eq $best) { $Time.AddDays(-1) } else { $best }
}

# Windows as start/end pairs, opening with the active one.
function Get-WindowList {
    param([DateTimeOffset]$Time, [int]$Count = 4)

    $upcoming = @(Get-NextSwitches -Time $Time -Count $Count)
    $start = Get-CurrentWindowStart $Time
    $kind = Get-PriceKind $Time
    $out = New-Object 'System.Collections.Generic.List[object]'

    for ($i = 0; $i -lt $Count; $i++) {
        $out.Add([pscustomobject]@{
            Start   = $start
            End     = $upcoming[$i].Time
            Kind    = $kind
            Current = ($i -eq 0)
        })
        $start = $upcoming[$i].Time
        $kind = $upcoming[$i].Kind
    }

    $out.ToArray()
}

# Compact countdown for the tray tooltip: 1h 23m, 4m, 42s.
function Format-Countdown {
    param([TimeSpan]$Span)

    if ($Span.Ticks -lt 0) {
        $Span = [TimeSpan]::Zero
    }
    if ($Span.TotalHours -ge 1) {
        return '{0}h {1:D2}m' -f [int][math]::Floor($Span.TotalHours), $Span.Minutes
    }
    if ($Span.TotalMinutes -ge 1) {
        return '{0}m' -f [int][math]::Floor($Span.TotalMinutes)
    }
    '{0}s' -f [int][math]::Floor($Span.TotalSeconds)
}

function Get-StatusText {
    param([string]$Kind)
    if ($Kind -eq 'peak') { 'Peak' } else { 'Off-peak' }
}

# --- Autostart shortcut -----------------------------------------------------
# Both the app and install.ps1 use these, so the on/off state has a single
# definition: the presence of the shortcut in the Startup folder.

$startupFolder = [Environment]::GetFolderPath([Environment+SpecialFolder]::Startup)
if ($startupFolder) {
    $script:StartupShortcut = Join-Path $startupFolder 'DeepSeek Price Clock.lnk'
}
else {
    # No Startup folder (for example on Linux, where the tests run).
    $script:StartupShortcut = $null
}

function Test-Autostart {
    if (-not $script:StartupShortcut) { return $false }
    Test-Path -LiteralPath $script:StartupShortcut
}

function Set-Autostart {
    param([string]$ScriptPath)

    if (-not $script:StartupShortcut) {
        throw 'No Startup folder on this system, so autostart cannot be set up.'
    }

    $shell = New-Object -ComObject WScript.Shell
    $shortcut = $shell.CreateShortcut($script:StartupShortcut)
    $shortcut.TargetPath = (Get-Process -Id $PID).Path
    $shortcut.Arguments = '-NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "{0}"' -f $ScriptPath
    $shortcut.WorkingDirectory = Split-Path -Parent $ScriptPath
    $shortcut.Description = 'DeepSeek peak and off-peak price clock'
    $shortcut.Save()
}

function Remove-Autostart {
    if (Test-Autostart) {
        Remove-Item -LiteralPath $script:StartupShortcut -Force
    }
}
