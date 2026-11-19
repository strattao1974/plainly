//! Plain-English explanations for the processes and services that most often make a PC feel slow.

pub struct Known {
    pub exe: &'static str,
    pub name: &'static str,
    pub why: &'static str,
    pub tip: Option<&'static str>,
    /// Part of Windows: never offer "End task".
    pub system: bool,
}

const fn k(exe: &'static str, name: &'static str, why: &'static str, tip: Option<&'static str>, system: bool) -> Known {
    Known { exe, name, why, tip, system }
}

const UPDATE_WHY: &str = "Windows is installing updates in the background. It can take a while, and it's best left to finish.";
const UPDATE_TIP: Option<&str> = Some("Leave the PC on and plugged in until it settles down.");
const DEFENDER_WHY: &str = "Your antivirus is scanning files. This is normal after big downloads, installs or updates, and usually calms down in a few minutes.";
const SEARCH_WHY: &str = "Windows is updating its search index so finding files stays fast. It's busiest after lots of new files arrive.";
const BROWSER_TIP: &str = "Each tab and extension runs separately, so lots of tabs add up. Close the ones you aren't using.";

pub const PROCESSES: &[Known] = &[
    k("msmpeng.exe", "Microsoft Defender", DEFENDER_WHY, None, true),
    k("mpdefendercoreservice.exe", "Microsoft Defender", DEFENDER_WHY, None, true),
    k("nissrv.exe", "Microsoft Defender (network protection)", DEFENDER_WHY, None, true),
    k("mssense.exe", "Microsoft Defender for Endpoint", DEFENDER_WHY, None, true),
    k("tiworker.exe", "Windows Update (installing)", UPDATE_WHY, UPDATE_TIP, true),
    k("trustedinstaller.exe", "Windows Update (installing)", UPDATE_WHY, UPDATE_TIP, true),
    k("mousocoreworker.exe", "Windows Update", "Windows is checking for, downloading or preparing updates.", UPDATE_TIP, true),
    k("usocoreworker.exe", "Windows Update", "Windows is checking for, downloading or preparing updates.", UPDATE_TIP, true),
    k("wuauclt.exe", "Windows Update", "Windows is checking for, downloading or preparing updates.", UPDATE_TIP, true),
    k("searchindexer.exe", "Windows Search indexer", SEARCH_WHY, Some("It backs off by itself when you're using the PC."), true),
    k("searchprotocolhost.exe", "Windows Search indexer", SEARCH_WHY, None, true),
    k("searchfilterhost.exe", "Windows Search indexer", SEARCH_WHY, None, true),
    k("searchhost.exe", "Windows Search (Start menu)", "The search box in the Start menu and taskbar.", None, true),
    k("searchapp.exe", "Windows Search (Start menu)", "The search box in the Start menu and taskbar.", None, true),
    k("compattelrunner.exe", "Microsoft Compatibility Telemetry", "Windows is checking your apps for compatibility and collecting diagnostic data. It runs now and then and finishes on its own.", None, true),
    k("wmiprvse.exe", "WMI Provider Host", "Another program is asking Windows for system information. If this stays busy, a monitoring or manufacturer utility is usually behind it.", None, true),
    k("memory compression", "Memory compression", "Windows is squeezing memory because RAM is nearly full.", Some("Close some apps or browser tabs to give it room."), true),
    k("system", "Windows (System)", "Windows itself: usually drivers, or lots of disk activity.", None, true),
    k("registry", "Windows registry", "Windows' settings database.", None, true),
    k("dwm.exe", "Desktop Window Manager", "Draws everything you see on screen. Heavy use usually means animations, several monitors, or a graphics driver problem.", Some("Updating your graphics driver often fixes this."), true),
    k("explorer.exe", "File Explorer and taskbar", "Runs your desktop, taskbar and folder windows. Heavy use often means it's busy with a big folder or a network drive.", None, true),
    k("csrss.exe", "Windows core process", "A core part of Windows.", None, true),
    k("lsass.exe", "Windows sign-in and security", "Handles sign-in and security checks.", None, true),
    k("services.exe", "Windows service manager", "Starts and stops background services.", None, true),
    k("audiodg.exe", "Windows Audio", "Processes sound. Heavy use often comes from audio enhancements or effects.", Some("Turning off audio enhancements in Sound settings can help."), true),
    k("spoolsv.exe", "Print Spooler", "Handles printing. A stuck print job can keep it busy.", Some("Clear the print queue in Printers & scanners."), true),
    k("msiexec.exe", "Windows Installer", "A program is being installed, updated or removed.", Some("Let it finish before restarting."), true),
    k("smartscreen.exe", "Microsoft SmartScreen", "Checking a file you downloaded or opened for safety.", None, true),
    k("backgroundtaskhost.exe", "Background tasks", "A Microsoft Store app is doing work in the background.", None, true),
    k("runtimebroker.exe", "Runtime Broker", "Manages permissions for Store apps. Heavy use points to one of those apps misbehaving.", None, true),
    k("sihost.exe", "Shell Infrastructure Host", "Part of the Windows desktop.", None, true),
    k("ctfmon.exe", "Text input", "Handles keyboard layouts, handwriting and voice typing.", None, true),
    k("securityhealthservice.exe", "Windows Security", "Keeps Windows Security's status up to date.", None, true),
    k("widgets.exe", "Widgets", "The Windows Widgets board (news and weather).", None, false),
    k("phoneexperiencehost.exe", "Phone Link", "Keeps your phone connected to the PC.", None, false),
    k("onedrive.exe", "OneDrive", "OneDrive is syncing files with the cloud.", Some("You can pause syncing for 2, 8 or 24 hours from the OneDrive icon."), false),
    k("dropbox.exe", "Dropbox", "Dropbox is syncing files.", Some("You can pause syncing from the Dropbox icon."), false),
    k("googledrivefs.exe", "Google Drive", "Google Drive is syncing files.", Some("You can pause syncing from the Google Drive icon."), false),
    k("chrome.exe", "Google Chrome", "Your web browser.", Some("Shift+Esc in Chrome shows which tab is the problem. Each tab and extension runs separately, so lots of tabs add up."), false),
    k("msedge.exe", "Microsoft Edge", "Your web browser.", Some("Shift+Esc in Edge shows which tab is the problem. Each tab and extension runs separately, so lots of tabs add up."), false),
    k("msedgewebview2.exe", "Edge WebView", "A web page shown inside another app (Teams, Outlook, widgets and many others).", None, false),
    k("firefox.exe", "Firefox", "Your web browser.", Some("Type about:processes in the address bar to see which tab is the problem."), false),
    k("brave.exe", "Brave", "Your web browser.", Some(BROWSER_TIP), false),
    k("opera.exe", "Opera", "Your web browser.", Some(BROWSER_TIP), false),
    k("vivaldi.exe", "Vivaldi", "Your web browser.", Some(BROWSER_TIP), false),
    k("ms-teams.exe", "Microsoft Teams", "Teams is busy. Calls, video and screen sharing are demanding.", Some("Turning off background effects in calls helps a lot."), false),
    k("teams.exe", "Microsoft Teams", "Teams is busy. Calls, video and screen sharing are demanding.", Some("Turning off background effects in calls helps a lot."), false),
    k("zoom.exe", "Zoom", "Video calls are demanding, especially with virtual backgrounds or effects.", Some("Turning off virtual backgrounds helps a lot."), false),
    k("outlook.exe", "Outlook", "Outlook may be syncing or indexing your mail.", None, false),
    k("olk.exe", "Outlook", "Outlook may be syncing your mail.", None, false),
    k("winword.exe", "Microsoft Word", "Microsoft Word.", None, false),
    k("excel.exe", "Microsoft Excel", "Microsoft Excel. Big spreadsheets with lots of formulas are demanding.", None, false),
    k("powerpnt.exe", "Microsoft PowerPoint", "Microsoft PowerPoint.", None, false),
    k("steam.exe", "Steam", "Steam may be downloading or updating games.", Some("Check Steam's Downloads page; you can pause it there."), false),
    k("steamwebhelper.exe", "Steam", "Steam's built-in browser.", None, false),
    k("epicgameslauncher.exe", "Epic Games Launcher", "May be downloading or updating games.", None, false),
    k("discord.exe", "Discord", "Discord: voice, video and screen sharing are demanding.", None, false),
    k("spotify.exe", "Spotify", "Spotify music player.", None, false),
    k("code.exe", "Visual Studio Code", "The code editor and its extensions.", None, false),
    k("vmmem", "Virtual machine (WSL, Docker or Hyper-V)", "A virtual machine is running and has memory and processor set aside for it.", Some("Shut it down when you're not using it (for WSL: wsl --shutdown)."), false),
    k("vmmemwsl", "Virtual machine (WSL)", "WSL is running and has memory and processor set aside for it.", Some("Run wsl --shutdown when you're not using it."), false),
    k("com.docker.backend.exe", "Docker Desktop", "Docker is running containers.", None, false),
    k("ollama.exe", "Ollama (local AI)", "Running an AI model on your own PC is very demanding.", None, false),
    k("python.exe", "Python", "A Python program or script.", None, false),
    k("node.exe", "Node.js", "A JavaScript program, often a developer tool.", None, false),
    k("obs64.exe", "OBS Studio", "Recording or streaming video is demanding.", None, false),
    k("ffmpeg.exe", "FFmpeg (video converter)", "Converting video is very demanding.", None, false),
    k("handbrake.exe", "HandBrake (video converter)", "Converting video is very demanding.", None, false),
];

pub struct KnownService {
    pub name: &'static str,
    pub label: &'static str,
    pub why: &'static str,
}

const fn s(name: &'static str, label: &'static str, why: &'static str) -> KnownService {
    KnownService { name, label, why }
}

pub const SERVICES: &[KnownService] = &[
    s("wuauserv", "Windows Update", UPDATE_WHY),
    s("usosvc", "Windows Update", "Windows is organising and installing updates."),
    s("dosvc", "Delivery Optimization", "Windows is downloading updates, and may be sharing them with other PCs to save bandwidth."),
    s("bits", "Background downloads", "Windows or an app is downloading something in the background."),
    s("wsearch", "Windows Search", SEARCH_WHY),
    s("diagtrack", "Telemetry", "Windows is collecting and sending diagnostic data."),
    s("sysmain", "SysMain", "Preloads the apps you use most so they open faster. Busy for a while after starting the PC."),
    s("windefend", "Microsoft Defender", DEFENDER_WHY),
    s("winmgmt", "WMI", "Another program is asking Windows for system information."),
    s("schedule", "Task Scheduler", "A scheduled task is running."),
    s("eventlog", "Event Log", "Windows is writing to its logs. Constant activity suggests something is logging lots of errors."),
    s("audiosrv", "Windows Audio", "Handles sound."),
    s("spooler", "Print Spooler", "Handles printing."),
    s("appxsvc", "Store app installs", "Store apps are being installed or updated."),
    s("installservice", "Microsoft Store installs", "The Microsoft Store is installing or updating apps."),
    s("clipsvc", "Store licensing", "Checks licences for Store apps."),
    s("trustedinstaller", "Windows Modules Installer", UPDATE_WHY),
    s("dnscache", "DNS Client", "Looks up website addresses."),
    s("cryptsvc", "Cryptographic Services", "Checks digital signatures, often during updates."),
    s("wpnservice", "Notifications", "Delivers notifications."),
    s("fontcache", "Font cache", "Speeds up fonts; busy after installing fonts."),
    s("netsvcs", "Windows networking", "Network-related Windows services."),
];

pub fn process(exe_lower: &str) -> Option<&'static Known> {
    PROCESSES.iter().find(|k| k.exe == exe_lower)
}

pub fn service(name_lower: &str) -> Option<&'static KnownService> {
    SERVICES.iter().find(|s| s.name == name_lower)
}

/// Windows parts that must never be offered "End task", even if not in the list above.
pub fn is_protected(exe_lower: &str) -> bool {
    const CRITICAL: &[&str] = &[
        "system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "winlogon.exe", "services.exe", "lsass.exe",
        "svchost.exe", "dwm.exe", "fontdrvhost.exe", "memory compression", "secure system", "lsaiso.exe",
        "msmpeng.exe", "explorer.exe", "plainly.exe",
    ];
    CRITICAL.contains(&exe_lower) || process(exe_lower).map(|k| k.system).unwrap_or(false)
}
