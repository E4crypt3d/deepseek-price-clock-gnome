# Tests for the pricing logic. Plain PowerShell, no Pester needed:
#   powershell -NoProfile -ExecutionPolicy Bypass -File .\tests\PriceClock.Tests.ps1
# Exit code is the number of failures (0 when everything passes).

. (Join-Path $PSScriptRoot '..\PriceClock.ps1')

$script:Failures = 0
$script:Passes = 0

function Assert-Equal {
    param($Expected, $Actual, [string]$Name)
    if ("$Expected" -eq "$Actual") {
        $script:Passes++
    }
    else {
        $script:Failures++
        Write-Host "FAIL  $Name`n      expected: $Expected`n      actual:   $Actual" -ForegroundColor Red
    }
}

function Assert-True {
    param([bool]$Condition, [string]$Name)
    Assert-Equal $true $Condition $Name
}

function At {
    param([string]$Iso)
    [DateTimeOffset]::Parse($Iso, [System.Globalization.CultureInfo]::InvariantCulture,
        [System.Globalization.DateTimeStyles]::AssumeUniversal)
}

# --- Which half of the pricing ---------------------------------------------

Assert-Equal 'peak' (Get-PriceKind (At '2026-11-24T02:00:00Z')) 'Tuesday 02:00 UTC is peak'
Assert-Equal 'peak' (Get-PriceKind (At '2026-11-24T03:30:00Z')) 'Tuesday 03:30 UTC is still in the 01:00 peak window'
Assert-Equal 'peak' (Get-PriceKind (At '2026-11-24T06:00:00Z')) 'Tuesday 06:00 UTC is peak'
Assert-Equal 'peak' (Get-PriceKind (At '2026-11-24T09:59:00Z')) 'Tuesday 09:59 UTC is peak'
Assert-Equal 'off' (Get-PriceKind (At '2026-11-24T10:00:00Z')) 'Tuesday 10:00 UTC is off-peak'
Assert-Equal 'off' (Get-PriceKind (At '2026-11-24T00:59:00Z')) 'Tuesday 00:59 UTC is off-peak'

# Weekends are off-peak all day, but only since 2026-08-22 16:00 UTC.
Assert-Equal 'off' (Get-PriceKind (At '2026-11-21T05:00:00Z')) 'Saturday after the rule is off-peak all day'
Assert-Equal 'peak' (Get-PriceKind (At '2026-08-15T02:00:00Z')) 'Saturday before the rule still has a peak window'
Assert-Equal 'peak' (Get-PriceKind (At '2026-08-16T02:00:00Z')) 'Sunday before the rule still has a peak window'
Assert-Equal 'off' (Get-PriceKind (At '2026-08-23T02:00:00Z')) 'Sunday after the rule is off-peak all day'
Assert-Equal 'off' (Get-PriceKind (At '2026-08-22T16:00:00Z')) 'The rule starts at 16:00 UTC'
Assert-Equal 'off' (Get-PriceKind (At '2026-08-23T10:00:00Z')) 'Sunday is off-peak all day'

# Chinese public holidays, by Beijing date.
Assert-Equal 'off' (Get-PriceKind (At '2026-10-02T02:00:00Z')) 'National Day is off-peak all day'
Assert-Equal 'off' (Get-PriceKind (At '2026-10-07T23:59:00Z')) 'Last minute of National Day is off-peak'
Assert-Equal 'peak' (Get-PriceKind (At '2026-10-08T01:30:00Z')) 'The day after National Day is peak again'
Assert-Equal 'off' (Get-PriceKind (At '2027-02-08T06:00:00Z')) 'Spring Festival 2027 is off-peak'
Assert-Equal 'off' (Get-PriceKind (At '2027-06-09T02:00:00Z')) 'Dragon Boat 2027 is off-peak'

# --- Next switch ------------------------------------------------------------

$next = Get-NextSwitch (At '2026-11-24T02:00:00Z')
Assert-Equal '2026-11-24T04:00:00Z' $next.Time.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Next switch after 02:00 is 04:00'
Assert-Equal 'off' $next.Kind 'The 04:00 switch turns pricing off'

$next = Get-NextSwitch (At '2026-11-26T09:59:00Z')
Assert-Equal '2026-11-26T10:00:00Z' $next.Time.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Next switch at 09:59 is 10:00'

# Friday after the last window the next real change is Monday 01:00 UTC.
$next = Get-NextSwitch (At '2026-11-27T10:30:00Z')
Assert-Equal '2026-11-30T01:00:00Z' $next.Time.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Weekend switches are skipped'
Assert-Equal 'peak' $next.Kind 'Monday 01:00 opens a peak window'

# A holiday stretch is skipped the same way.
$next = Get-NextSwitch (At '2026-09-30T11:00:00Z')
Assert-Equal '2026-10-08T01:00:00Z' $next.Time.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Holiday switches are skipped'

# --- Window start -----------------------------------------------------------

$start = Get-CurrentWindowStart (At '2026-11-24T03:30:00Z')
Assert-Equal '2026-11-24T01:00:00Z' $start.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Window start before 04:00 is 01:00'

$start = Get-CurrentWindowStart (At '2026-11-24T00:30:00Z')
Assert-Equal '2026-11-23T10:00:00Z' $start.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Night window starts at the previous 10:00'

$start = Get-CurrentWindowStart (At '2026-11-21T05:00:00Z')
Assert-Equal '2026-11-20T10:00:00Z' $start.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Saturday window starts Friday 10:00'

$start = Get-CurrentWindowStart (At '2026-11-29T05:00:00Z')
Assert-Equal '2026-11-27T10:00:00Z' $start.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Sunday window starts Friday 10:00'

# --- Window list ------------------------------------------------------------

$now = At '2026-11-24T02:30:00Z'
$windows = @(Get-WindowList -Time $now -Count 5)
Assert-Equal 5 $windows.Count 'Window list returns the requested count'
Assert-Equal 'peak' $windows[0].Kind 'First window is the active one'
Assert-Equal $true $windows[0].Current 'First window is marked current'
Assert-Equal '2026-11-24T04:00:00Z' $windows[0].End.ToString('yyyy-MM-ddTHH:mm:ssZ') 'Active window ends at the next switch'
Assert-Equal 'off' $windows[1].Kind 'Second window is off-peak'
for ($i = 0; $i -lt $windows.Count - 1; $i++) {
    Assert-Equal $windows[$i].End.ToString('o') $windows[$i + 1].Start.ToString('o') "Window $i is contiguous"
}

# --- Countdown --------------------------------------------------------------

Assert-Equal '1h 23m' (Format-Countdown ([TimeSpan]::FromMinutes(83))) 'Countdown over an hour'
Assert-Equal '4m' (Format-Countdown ([TimeSpan]::FromSeconds(270))) 'Countdown under an hour'
Assert-Equal '42s' (Format-Countdown ([TimeSpan]::FromSeconds(42))) 'Countdown under a minute'
Assert-Equal '0s' (Format-Countdown ([TimeSpan]::FromSeconds(-5))) 'Negative countdown clamps to zero'

# --- Invariants over two years ---------------------------------------------

$cursor = At '2026-01-01T00:00:00Z'
$checked = 0
$bad = 0
$badSample = ''

while ($cursor -lt (At '2028-01-01T00:00:00Z')) {
    $kind = Get-PriceKind $cursor
    $upcoming = @(Get-NextSwitches -Time $cursor -Count 4)
    $first = $upcoming[0]

    $ok = $first.Time -gt $cursor
    $ok = $ok -and ($first.Kind -ne $kind)
    $ok = $ok -and (-not (Test-OffPeakAllDay $first.Time))
    $minute = $first.Time.UtcDateTime.Hour * 60 + $first.Time.UtcDateTime.Minute
    $ok = $ok -and ($script:SwitchMinutes -contains $minute)
    for ($i = 0; $i -lt $upcoming.Count - 1; $i++) {
        $ok = $ok -and ($upcoming[$i].Time -lt $upcoming[$i + 1].Time)
        $ok = $ok -and ($upcoming[$i].Kind -ne $upcoming[$i + 1].Kind)
    }

    if (-not $ok) {
        $bad++
        if ($badSample -eq '') { $badSample = $cursor.ToString('o') }
    }
    $checked++
    $cursor = $cursor.AddHours(1)
}

Assert-Equal 0 $bad "Every hourly sample in 2026-2027 has a sane next switch (first bad: $badSample)"
Assert-Equal 17520 $checked 'Two years of hourly samples were checked'

Write-Host ''
Write-Host ("{0} passed, {1} failed" -f $script:Passes, $script:Failures)
exit $script:Failures
