# DeepSeek Price Clock for Windows 11

The Windows 11 side of [deepseek-price-clock-gnome](https://github.com/E4crypt3d/deepseek-price-clock-gnome):
a tray icon that shows whether DeepSeek's API is billing at peak or off-peak
rates, how long is left in the current window, and when the next switches are.

No installer, no admin rights, no dependencies, no background service. It ships
two ways: `DeepSeekPriceClock.exe`, a small native binary in `rust/`, and the
original PowerShell script that runs on the PowerShell 5.1 and .NET Framework
that ship with Windows 11. Both show the same dot and read the same rules.

The dot is green when off-peak, red when peak. Hover it for the countdown, click
it for the full schedule and rates.

## Why a tray icon

Windows 11 has no supported way for a normal app to put a live widget in the
taskbar. Third-party widgets live in the Widgets board and require a packaged
MSIX app with Adaptive Cards, which is far heavier than this needs. The
notification area is the lightweight supported surface, so that is what this
uses. Notifications are raised as balloon tips, which Windows 11 shows as normal
toasts in the Action Center.

## Install

Download `DeepSeekPriceClock.exe` straight from the release page, or take the
release zip (which also carries the PowerShell build and the tests) and run:

```powershell
.\windows\DeepSeekPriceClock.exe
```

That is the whole install. Use **Start with Windows** in its menu to have it come
back at login (a per-user `Run` value, no admin rights), and **Exit** to stop it.
The indicator re-adds itself if Explorer restarts, so it does not disappear for
the rest of the session. Both builds take the same single-instance lock, so at
login only one dot appears even if both autostarts were set, but pick one anyway.

Or use the PowerShell version, from a release zip or from the repo:

```powershell
# release zip: unpacks to a windows\ folder
Expand-Archive deepseek-price-clock-windows.zip -DestinationPath .
cd windows

# or from source
git clone https://github.com/E4crypt3d/deepseek-price-clock-gnome.git
cd deepseek-price-clock-gnome\windows

powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -StartNow
```

That adds a shortcut to your Startup folder and starts the clock. To start it
without autostart:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\DeepSeekPriceClock.ps1
```

`-ExecutionPolicy Bypass` is needed because the script is not code signed. It
does not change any system setting, it applies to that one process.

To remove the autostart entry:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1 -Uninstall
```

## Using it

- Left or right click the dot for the menu.
- The header shows the current pricing and when it ends.
- **Next switches** lists the next four price changes with countdowns.
- **Rates per 1M tokens** shows the off-peak and peak prices side by side.
- **Start with Windows** toggles the Startup shortcut.
- **Notify when prices switch** shows a toast at each price change.
- **Exit** stops it.

Only one instance runs at a time, so autostart plus a manual start will not give
you two dots.

## Pricing rules

The same rules as the GNOME extension, ported from
[deepakness/deepseek-price-clock](https://github.com/deepakness/deepseek-price-clock):

- Peak windows are 01:00 to 04:00 and 06:00 to 10:00 UTC. Everything else is
  off-peak.
- Since 16:00 UTC on 22 August 2026, weekends and Chinese public holidays are
  off-peak all day. Before that date the split applied every day.
- Beijing time decides what counts as a weekend or holiday, which is why the
  dates in `PriceClock.ps1` are Beijing dates and the switch times are UTC.

The 2027 holiday dates come from the State Council notice published in November
2026. Add the next year's dates to `ChinaHolidays` when they are announced.

## Tests

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\tests\PriceClock.Tests.ps1
```

42 checks, no Pester needed: known instants either side of every boundary, the
weekend rule start, holiday stretches, window starts, the window list, countdown
formatting, plus a two year hourly invariant sweep.

The native build has its own tests. They share the pricing module with the app and
run on any platform, no Windows needed. The rules are compared against the
JavaScript implementation used by the GNOME extension for 17,520 hourly instants
over 2026 and 2027, with identical results.

```bash
cargo test --manifest-path windows/rust/Cargo.toml --lib
cargo build --release --target x86_64-pc-windows-gnu \
  --manifest-path windows/rust/Cargo.toml
```

The release workflow runs these Rust tests and the PowerShell suite on a Windows
runner, and the GNOME tests on Linux, before it builds the `.exe` into the Windows
zip.

## Files

| File | What it is |
| --- | --- |
| `DeepSeekPriceClock.exe` | The native tray app, built from `rust/` (in the release zip) |
| `DeepSeekPriceClock.ps1` | The PowerShell tray app: icon, menu, timer, notifications |
| `PriceClock.ps1` | Pricing rules and the autostart helpers, no UI |
| `install.ps1` | Adds or removes the Startup shortcut |
| `tests/PriceClock.Tests.ps1` | Logic tests |
| `rust/` | The native source: `pricing.rs` rules, `main.rs` Win32 tray |

## Notes

- Windows 11 puts new tray icons in the overflow chevron until you drag the dot
  onto the taskbar; after that it stays visible.
- The dot is drawn in code in both builds, so the repo has no image assets.
- The timer fires every 20 seconds, which is enough to keep a minute countdown
  accurate and to catch a switch promptly. It does nothing in between.
- The native build is one hidden window, one timer and two icons drawn once;
  the menu is built only while it is open and destroyed when it closes.
- Nothing leaves your machine and nothing is fetched from the network apart
  from the pricing page if you click it.

## Credits

Pricing logic and the peak/off-peak rules come from
[deepakness/deepseek-price-clock](https://github.com/deepakness/deepseek-price-clock),
MIT (c) 2026 deepakness. This is an independent Windows port, not affiliated
with DeepSeek.

## License

MIT, see [LICENSE](LICENSE).
