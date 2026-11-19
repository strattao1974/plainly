param([string]$Dir)
New-Item -ItemType Directory -Force $Dir | Out-Null
$today = (Get-Date).Date
function At([int]$daysAgo, [int]$h, [int]$m) { [DateTimeOffset]::new($today.AddDays(-$daysAgo).AddHours($h).AddMinutes($m)).ToUnixTimeSeconds() }
function Open($label, $target) { @{ Open = @{ label = $label; target = $target } } }
function Run($label, $exe, $args2) { @{ Run = @{ label = $label; exe = $exe; args = $args2 } } }
function Page($label, $page) { @{ Page = @{ label = $label; page = $page } } }

$events = @(
  @{ time = (At 3 9 0); kind = 'Info'; title = 'Plainly started keeping watch'; detail = "From now on, new startup apps, background services, scheduled tasks, installs, changed default apps, updates, restarts, crashes, slowdowns and internet drop-outs will show up here. Anything older comes from Windows' own records."; actions = @() },
  @{ time = (At 2 13 10); kind = 'Threat'; title = 'Microsoft Defender found a threat'; detail = 'Microsoft Defender spotted Trojan:Win32/Wacatac.B!ml. Found in: Downloads\free-video-converter-setup.exe'; actions = @((Open 'Open Windows Security' 'windowsdefender://threat')) },
  @{ time = (At 2 19 45); kind = 'Disk'; title = 'Drive C: lost 18 GB of free space since yesterday'; detail = '41 GB is now free out of 476 GB. Big drops usually come from updates, downloads, games, or apps keeping large caches.'; actions = @((Open 'Storage settings' 'ms-settings:storagesense')) },
  @{ time = (At 1 11 2); kind = 'Crash'; title = 'Google Chrome crashed 3 times'; detail = 'It closed unexpectedly. If this keeps happening, updating or reinstalling the app usually helps.'; actions = @((Run 'Reliability history' 'perfmon.exe' '/rel')) },
  @{ time = (At 1 14 30); kind = 'Program'; title = 'Installed: Adobe Acrobat (64-bit)'; detail = 'Version 25.001.20432. Made by Adobe.'; actions = @((Open 'Apps settings' 'ms-settings:appsfeatures')) },
  @{ time = (At 1 14 31); kind = 'Service'; title = 'New background service: Adobe Genuine Monitor Service'; detail = 'Services run in the background without a window. This one starts automatically when Windows starts. Program: "C:\Program Files (x86)\Common Files\Adobe\AdobeGCClient\AGMService.exe"'; actions = @((Run 'Open Services' 'services.msc' '')) },
  @{ time = (At 1 16 14); kind = 'Slowdown'; title = 'Your PC was struggling for 6 minutes'; detail = 'From 4:14 PM to 4:20 PM, the processor was flat out (peak 98%). Main cause: Windows Update (installing). Windows is installing updates in the background. It can take a while, and it''s best left to finish.'; actions = @((Page 'Why is it slow?' 'slow')) },
  @{ time = (At 0 3 5); kind = 'Update'; title = 'Windows Update installed 3 updates'; detail = "2026-11 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5068861)`nSecurity Intelligence Update for Microsoft Defender Antivirus (KB2267602)`nWindows Malicious Software Removal Tool x64 - v5.137 (KB890830)"; actions = @((Open 'Update history' 'ms-settings:windowsupdate-history')) },
  @{ time = (At 0 3 12); kind = 'Restart'; title = 'Windows Update restarted your PC'; detail = 'Requested by MoUsoCoreWorker.exe. Reason given: Operating System: Service pack (Planned).'; actions = @() },
  @{ time = (At 0 7 59); kind = 'Internet'; title = 'Your internet dropped out for 4 minutes'; detail = 'From 7:59 AM to 8:03 AM. Your router was reachable the whole time, so the problem was beyond it: your internet connection or provider. If this happens often, this timeline is useful evidence for them.'; actions = @((Page 'Check internet now' 'internet')) },
  @{ time = (At 0 9 15); kind = 'DefaultApp'; title = 'Your default web browser changed from Google Chrome to Microsoft Edge'; detail = "If you didn't choose this, an app or update switched it for you."; actions = @((Open "Change it back$([char]0x2026)" 'ms-settings:defaultapps')) },
  @{ time = (At 0 10 42); kind = 'Startup'; title = 'Zoom now starts with Windows'; detail = 'Added to your startup list. Every time you sign in it runs: "%APPDATA%\Zoom\bin\Zoom.exe" --autostart'; actions = @(@{ DisableStartup = @{ label = 'Turn it off'; hive = 'HKCU'; approved_key = 'Run'; name = 'ZoomDemo' } }, (Open "Startup apps$([char]0x2026)" 'ms-settings:startupapps')) }
)
$i = 1
$lines = foreach ($e in $events) { $e.id = $i; $i++; $e.approx = $false; $e.noise = $false; ($e | ConvertTo-Json -Depth 6 -Compress) }
[System.IO.File]::WriteAllLines((Join-Path $Dir 'timeline.jsonl'), [string[]]$lines, (New-Object System.Text.UTF8Encoding $false))
$status = @{ watching_since = (At 3 9 0); last_scan = [DateTimeOffset]::Now.ToUnixTimeSeconds() } | ConvertTo-Json -Compress
[System.IO.File]::WriteAllText((Join-Path $Dir 'status.json'), $status)
$settings = @{ welcomed = $true; told_about_tray = $true; last_viewed = (At 0 8 30); check_updates = $false } | ConvertTo-Json -Compress
[System.IO.File]::WriteAllText((Join-Path $Dir 'settings.json'), $settings)
"demo data written: $($events.Count) events"
