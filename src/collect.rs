//! Reads the current state of the PC into a Snapshot. Everything here works without admin rights.

use crate::snapshot::{Program, ServiceItem, Snapshot, StartupItem, StartupLoc};
use crate::winutil::{indirect_string, wide};
use std::collections::BTreeMap;
use std::path::PathBuf;
use winreg::enums::*;
use winreg::RegKey;

pub fn snapshot() -> Snapshot {
    Snapshot {
        startup: startup(),
        services: services(),
        tasks: tasks(),
        programs: programs(),
        defaults: defaults(),
    }
}

fn hkcu() -> RegKey {
    RegKey::predef(HKEY_CURRENT_USER)
}
fn hklm() -> RegKey {
    RegKey::predef(HKEY_LOCAL_MACHINE)
}

pub fn startup() -> BTreeMap<String, StartupItem> {
    let mut items = Vec::new();
    let run = r"Software\Microsoft\Windows\CurrentVersion\Run";
    read_run(&hkcu(), run, StartupLoc::UserRun, &mut items);
    read_run(&hklm(), run, StartupLoc::MachineRun, &mut items);
    read_run(&hklm(), r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run", StartupLoc::MachineRun32, &mut items);
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let p = PathBuf::from(appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        read_folder(p, StartupLoc::UserFolder, &mut items);
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        let p = PathBuf::from(pd).join(r"Microsoft\Windows\Start Menu\Programs\StartUp");
        read_folder(p, StartupLoc::MachineFolder, &mut items);
    }
    let mut map = BTreeMap::new();
    for mut it in items {
        it.enabled = approved(it.location, &it.name);
        map.insert(format!("{:?}|{}", it.location, it.name), it);
    }
    map
}

fn read_run(root: &RegKey, path: &str, loc: StartupLoc, out: &mut Vec<StartupItem>) {
    let Ok(key) = root.open_subkey_with_flags(path, KEY_READ) else { return };
    for (name, _) in key.enum_values().filter_map(Result::ok) {
        if name.is_empty() {
            continue;
        }
        let command = key.get_value::<String, _>(&name).unwrap_or_default();
        out.push(StartupItem { name, command, location: loc, enabled: true });
    }
}

fn read_folder(dir: PathBuf, loc: StartupLoc, out: &mut Vec<StartupItem>) {
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    for entry in rd.flatten() {
        let file = entry.file_name().to_string_lossy().to_string();
        if file.eq_ignore_ascii_case("desktop.ini") {
            continue;
        }
        let shown = file.strip_suffix(".lnk").unwrap_or(&file).to_string();
        out.push(StartupItem {
            name: shown,
            command: entry.path().display().to_string(),
            location: loc,
            enabled: true,
        });
    }
}

fn approved(loc: StartupLoc, name: &str) -> bool {
    let root = if loc.hive() == "HKLM" { hklm() } else { hkcu() };
    let path = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{}", loc.approved_key());
    let Ok(key) = root.open_subkey(path) else { return true };
    let value_name = match loc {
        StartupLoc::UserFolder | StartupLoc::MachineFolder => format!("{name}.lnk"),
        _ => name.to_string(),
    };
    let raw = key.get_raw_value(&value_name).or_else(|_| key.get_raw_value(name));
    match raw {
        Ok(v) if !v.bytes.is_empty() => v.bytes[0] & 1 == 0,
        _ => true,
    }
}

pub fn looks_like_windows(image: &str) -> bool {
    let l = image.to_ascii_lowercase();
    l.contains("\\windows\\")
        || l.contains("%systemroot%")
        || l.contains("%windir%")
        || l.starts_with("system32\\")
        || l.starts_with("\\systemroot\\")
        || l.contains("windows defender")
        || l.contains("\\microsoft\\edgeupdate\\")
}

pub fn services() -> BTreeMap<String, ServiceItem> {
    let mut map = BTreeMap::new();
    let Ok(root) = hklm().open_subkey(r"SYSTEM\CurrentControlSet\Services") else { return map };
    for name in root.enum_keys().filter_map(Result::ok) {
        let Ok(k) = root.open_subkey(&name) else { continue };
        let Ok(ty) = k.get_value::<u32, _>("Type") else { continue };
        // 0x10/0x20 are real services; 0x80 marks per-sign-in copies whose names change every session.
        if ty & 0x30 == 0 || ty & 0x80 != 0 {
            continue;
        }
        let image = k.get_value::<String, _>("ImagePath").unwrap_or_default();
        let start = k.get_value::<u32, _>("Start").unwrap_or(3);
        let display = k
            .get_value::<String, _>("DisplayName")
            .ok()
            .and_then(|d| indirect_string(&d))
            .unwrap_or_else(|| name.clone());
        let windows = looks_like_windows(&image);
        map.insert(name.to_ascii_lowercase(), ServiceItem { display, image, start, windows });
    }
    map
}

pub fn tasks() -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    unsafe {
        let _ = tasks_com(&mut map);
    }
    map
}

unsafe fn tasks_com(out: &mut BTreeMap<String, bool>) -> windows::core::Result<()> {
    use windows::core::{BSTR, VARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::TaskScheduler::{ITaskFolder, ITaskService, TaskScheduler, TASK_ENUM_HIDDEN};

    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
    svc.Connect(&VARIANT::default(), &VARIANT::default(), &VARIANT::default(), &VARIANT::default())?;
    let root = svc.GetFolder(&BSTR::from("\\"))?;

    fn walk(folder: &ITaskFolder, out: &mut BTreeMap<String, bool>, depth: u32) {
        if depth > 12 {
            return;
        }
        unsafe {
            if let Ok(tasks) = folder.GetTasks(TASK_ENUM_HIDDEN.0) {
                let n = tasks.Count().unwrap_or(0);
                for i in 1..=n {
                    if let Ok(t) = tasks.get_Item(&VARIANT::from(i)) {
                        if let Ok(path) = t.Path() {
                            let enabled = t.Enabled().map(|b| b.as_bool()).unwrap_or(true);
                            out.insert(path.to_string(), enabled);
                        }
                    }
                }
            }
            if let Ok(folders) = folder.GetFolders(0) {
                let n = folders.Count().unwrap_or(0);
                for i in 1..=n {
                    if let Ok(f) = folders.get_Item(&VARIANT::from(i)) {
                        walk(&f, out, depth + 1);
                    }
                }
            }
        }
    }
    walk(&root, out, 0);
    Ok(())
}

pub fn programs() -> BTreeMap<String, Program> {
    let mut map = BTreeMap::new();
    let uninstall = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    read_uninstall(&hklm(), uninstall, &mut map);
    read_uninstall(&hklm(), r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", &mut map);
    read_uninstall(&hkcu(), uninstall, &mut map);
    read_store_apps(&mut map);
    map
}

fn read_uninstall(root: &RegKey, path: &str, out: &mut BTreeMap<String, Program>) {
    let Ok(key) = root.open_subkey(path) else { return };
    for sub in key.enum_keys().filter_map(Result::ok) {
        let Ok(k) = key.open_subkey(&sub) else { continue };
        let Ok(name) = k.get_value::<String, _>("DisplayName") else { continue };
        let name = name.trim().to_string();
        if name.is_empty()
            || k.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1
            || k.get_value::<String, _>("ParentKeyName").is_ok()
        {
            continue;
        }
        let release = k.get_value::<String, _>("ReleaseType").unwrap_or_default();
        if ["Update", "Hotfix", "Security Update"].contains(&release.as_str()) {
            continue;
        }
        let version = k.get_value::<String, _>("DisplayVersion").unwrap_or_default();
        let publisher = k.get_value::<String, _>("Publisher").unwrap_or_default();
        out.entry(name.clone()).or_insert(Program { name, version, publisher, store: false, microsoft: false });
    }
}

/// Programs whose recorded install date is within the last `days` days: (name, version, publisher, unix time).
pub fn recent_installs(days: i64) -> Vec<(String, String, String, i64)> {
    use chrono::{Local, NaiveDate, TimeZone};
    let cutoff = crate::journal::now() - days * 86_400;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let uninstall = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";
    let roots = [
        (hklm(), uninstall),
        (hklm(), r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (hkcu(), uninstall),
    ];
    for (root, path) in roots {
        let Ok(key) = root.open_subkey(path) else { continue };
        for sub in key.enum_keys().filter_map(Result::ok) {
            let Ok(k) = key.open_subkey(&sub) else { continue };
            let Ok(name) = k.get_value::<String, _>("DisplayName") else { continue };
            if k.get_value::<u32, _>("SystemComponent").unwrap_or(0) == 1 || k.get_value::<String, _>("ParentKeyName").is_ok() {
                continue;
            }
            let Ok(date) = k.get_value::<String, _>("InstallDate") else { continue };
            let Ok(d) = NaiveDate::parse_from_str(date.trim(), "%Y%m%d") else { continue };
            let Some(noon) = d.and_hms_opt(12, 0, 0) else { continue };
            let Some(t) = Local.from_local_datetime(&noon).single().map(|x| x.timestamp()) else { continue };
            if t >= cutoff && seen.insert(name.clone()) {
                out.push((
                    name.trim().to_string(),
                    k.get_value::<String, _>("DisplayVersion").unwrap_or_default(),
                    k.get_value::<String, _>("Publisher").unwrap_or_default(),
                    t,
                ));
            }
        }
    }
    out
}

const FRAMEWORK_PREFIXES: &[&str] = &[
    "microsoft.vclibs",
    "microsoft.net.",
    "microsoft.ui.xaml",
    "microsoft.windowsappruntime",
    "microsoft.services.store",
    "microsoft.directx",
    "microsoft.advertising",
    "microsoftwindows.",
    "windows.",
    "microsoft.windows.",
    "microsoft.languageexperiencepack",
];

fn read_store_apps(out: &mut BTreeMap<String, Program>) {
    let path = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
    let Ok(key) = hkcu().open_subkey(path) else { return };
    for full in key.enum_keys().filter_map(Result::ok) {
        let Some(pkg) = parse_package_name(&full) else { continue };
        let lower = pkg.family.to_ascii_lowercase();
        if FRAMEWORK_PREFIXES.iter().any(|p| lower.starts_with(p)) {
            continue;
        }
        let Ok(k) = key.open_subkey(&full) else { continue };
        let display = k
            .get_value::<String, _>("DisplayName")
            .ok()
            .and_then(|d| indirect_string(&d))
            .filter(|d| !d.starts_with("ms-resource:") && !d.starts_with('@'))
            .unwrap_or_else(|| pkg.family.clone());
        let entry_key = format!("store:{}", pkg.family);
        let candidate = Program {
            name: display,
            version: pkg.version.clone(),
            publisher: String::new(),
            store: true,
            microsoft: pkg.microsoft,
        };
        match out.get(&entry_key) {
            Some(existing) if version_ge(&existing.version, &candidate.version) => {}
            _ => {
                out.insert(entry_key, candidate);
            }
        }
    }
}

pub struct PackageName {
    pub family: String,
    pub version: String,
    pub microsoft: bool,
}

/// "Name_1.2.3.0_x64__8wekyb3d8bbwe" -> family "Name", version "1.2.3.0". Resource and language packs are skipped.
pub fn parse_package_name(full: &str) -> Option<PackageName> {
    let parts: Vec<&str> = full.split('_').collect();
    if parts.len() < 5 {
        return None;
    }
    let resource = parts[3];
    if resource.contains("split") || resource.contains("language") || resource.contains("scale") {
        return None;
    }
    let publisher = parts[parts.len() - 1];
    Some(PackageName {
        family: parts[0].to_string(),
        version: parts[1].to_string(),
        microsoft: publisher == "8wekyb3d8bbwe" || publisher == "cw5n1h2txyewy",
    })
}

fn version_ge(a: &str, b: &str) -> bool {
    let p = |s: &str| s.split('.').map(|x| x.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    p(a) >= p(b)
}

const DEFAULTS: &[(&str, &str)] = &[
    ("web browser", r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice"),
    ("email app", r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\mailto\UserChoice"),
    ("PDF app", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.pdf\UserChoice"),
    ("photo app (.jpg)", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.jpg\UserChoice"),
    ("photo app (.png)", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.png\UserChoice"),
    ("video player (.mp4)", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.mp4\UserChoice"),
    ("music player (.mp3)", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.mp3\UserChoice"),
    ("text editor (.txt)", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.txt\UserChoice"),
    ("zip app (.zip)", r"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\.zip\UserChoice"),
];

pub fn defaults() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for (label, path) in DEFAULTS {
        let Ok(k) = hkcu().open_subkey(path) else { continue };
        let Ok(progid) = k.get_value::<String, _>("ProgId") else { continue };
        map.insert(label.to_string(), app_name_for_progid(&progid));
    }
    map
}

const KNOWN_PROGIDS: &[(&str, &str)] = &[
    ("chromehtml", "Google Chrome"),
    ("chromepdf", "Google Chrome"),
    ("msedgehtm", "Microsoft Edge"),
    ("msedgepdf", "Microsoft Edge"),
    ("msedgemht", "Microsoft Edge"),
    ("firefoxurl", "Firefox"),
    ("firefoxhtml", "Firefox"),
    ("firefoxpdf", "Firefox"),
    ("bravehtml", "Brave"),
    ("bravefile", "Brave"),
    ("operastable", "Opera"),
    ("vivaldihtm", "Vivaldi"),
    ("ie.http", "Internet Explorer"),
    ("acroexch", "Adobe Acrobat"),
    ("acrobat.document", "Adobe Acrobat"),
    ("appxq0fevzme2pys62n3e0fbqa7peapykr8v", "Microsoft Edge"),
    ("appx90nv6nhay5n6a98fnetv7tpk64pp35es", "Microsoft Edge"),
    ("appxvcc3jw1zy9cb2pqkhexpyhgvd1gcd7nd", "Photos"),
    ("appx43hnxtbyyps62jhe9sqpdzxn1790zetc", "Photos"),
    ("appx6eg8h5sxqq90pv53845wmnbewywdqq5h", "Media Player"),
    ("appxk0g4vb8gvt7b93tg50ybcy892pge6jmt", "Media Player"),
    ("txtfile", "Notepad"),
    ("compressedfolder", "File Explorer"),
    ("applications\\notepad.exe", "Notepad"),
];

pub fn app_name_for_progid(progid: &str) -> String {
    let lower = progid.to_ascii_lowercase();
    for (prefix, name) in KNOWN_PROGIDS {
        if lower.starts_with(prefix) {
            return name.to_string();
        }
    }
    let hkcr = RegKey::predef(HKEY_CLASSES_ROOT);
    if let Ok(app) = hkcr.open_subkey(format!(r"{progid}\Application")) {
        if let Some(n) = app.get_value::<String, _>("ApplicationName").ok().and_then(|n| indirect_string(&n)) {
            if !n.starts_with('@') && !n.is_empty() {
                return n;
            }
        }
    }
    if let Some(n) = progid_exe_description(&hkcr, progid) {
        return n;
    }
    if let Ok(k) = hkcr.open_subkey(progid) {
        if let Some(n) = k
            .get_value::<String, _>("FriendlyTypeName")
            .ok()
            .and_then(|n| indirect_string(&n))
            .or_else(|| k.get_value::<String, _>("").ok())
        {
            if !n.is_empty() && !n.starts_with('@') {
                return n;
            }
        }
    }
    if let Some(exe) = lower.strip_prefix("applications\\") {
        return exe.trim_end_matches(".exe").to_string();
    }
    progid.to_string()
}

/// The opening program's own name, e.g. "VLC media player", from the progid's open command.
fn progid_exe_description(hkcr: &RegKey, progid: &str) -> Option<String> {
    let cmd = hkcr
        .open_subkey(format!(r"{progid}\shell\open\command"))
        .ok()?
        .get_value::<String, _>("")
        .ok()?;
    let exe = crate::winutil::exe_from_command(&expand_env(&cmd))?;
    let lower = exe.to_ascii_lowercase();
    if lower.ends_with("rundll32.exe") || lower.ends_with("explorer.exe") {
        return None;
    }
    crate::winutil::file_description(&exe)
}

fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                out.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

pub struct DriveSpace {
    pub root: String,
    pub free: u64,
    pub total: u64,
}

pub fn drives() -> Vec<DriveSpace> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW};
    const DRIVE_FIXED: u32 = 3;
    let mut out = Vec::new();
    for letter in b'C'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let w = wide(&root);
        unsafe {
            if GetDriveTypeW(PCWSTR(w.as_ptr())) != DRIVE_FIXED {
                continue;
            }
            let mut free = 0u64;
            let mut total = 0u64;
            if GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), Some(&mut total), None).is_ok() && total > 0 {
                out.push(DriveSpace { root: root[..2].to_string(), free, total });
            }
        }
    }
    out
}

pub fn reboot_pending() -> bool {
    let k = hklm();
    k.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired").is_ok()
        || k.open_subkey(r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_store_package_names() {
        let p = parse_package_name("SpotifyAB.SpotifyMusic_1.250.0.0_x64__zpdnekdrzrea0").unwrap();
        assert_eq!(p.family, "SpotifyAB.SpotifyMusic");
        assert_eq!(p.version, "1.250.0.0");
        assert!(!p.microsoft);
        let m = parse_package_name("Microsoft.WindowsCalculator_11.2405.2.0_x64__8wekyb3d8bbwe").unwrap();
        assert!(m.microsoft);
        assert!(parse_package_name("Microsoft.Foo_1.0.0.0_neutral_split.scale-200_8wekyb3d8bbwe").is_none());
        assert!(parse_package_name("garbage").is_none());
    }

    #[test]
    fn windows_paths_are_recognised() {
        assert!(looks_like_windows(r"%SystemRoot%\system32\svchost.exe -k netsvcs"));
        assert!(looks_like_windows(r"C:\WINDOWS\system32\spoolsv.exe"));
        assert!(looks_like_windows(r#""C:\ProgramData\Microsoft\Windows Defender\platform\4.18\MsMpEng.exe""#));
        assert!(!looks_like_windows(r#""C:\Program Files\Acme\acme.exe""#));
    }

    #[test]
    fn version_comparison_is_numeric() {
        assert!(version_ge("1.10.0", "1.9.5"));
        assert!(!version_ge("1.2", "1.10"));
    }

    #[test]
    #[ignore = "reads the real PC; run with --ignored --nocapture"]
    fn live_snapshot() {
        let t = std::time::Instant::now();
        let s = snapshot();
        println!("snapshot took {:?}", t.elapsed());
        println!("startup {}: {:?}", s.startup.len(), s.startup.values().map(|i| (&i.name, i.enabled)).collect::<Vec<_>>());
        println!("services {} (non-windows {})", s.services.len(), s.services.values().filter(|x| !x.windows).count());
        println!("non-windows services: {:?}", s.services.values().filter(|x| !x.windows).map(|x| &x.display).take(15).collect::<Vec<_>>());
        println!("tasks {} (non-ms {:?})", s.tasks.len(), s.tasks.keys().filter(|k| !crate::snapshot::is_windows_task(k)).take(10).collect::<Vec<_>>());
        println!("programs {} (store {})", s.programs.len(), s.programs.values().filter(|p| p.store).count());
        println!("defaults {:?}", s.defaults);
        println!("recent installs {:?}", recent_installs(14).iter().map(|x| &x.0).collect::<Vec<_>>());
        for d in drives() {
            println!("drive {} free {} of {}", d.root, d.free, d.total);
        }
        println!("reboot pending {}", reboot_pending());
        for src in crate::eventlog::SOURCES {
            let raw = crate::eventlog::query_since(src.channel, src.filter, None, 14);
            println!("{}: {} raw, latest id {:?}", src.channel, raw.len(), crate::eventlog::latest_record_id(src.channel));
            for e in crate::eventlog::to_events(&raw).iter().take(4) {
                println!("   {} | {}", e.title, e.detail.lines().next().unwrap_or(""));
            }
        }
        let f = crate::slow::gather(5);
        let r = crate::slow::build(&f);
        println!("{}", r.to_text());
        let ad = crate::net::adapters();
        println!("adapters {:?}", ad.iter().map(|a| (&a.name, a.wifi, a.gateway, &a.dns)).collect::<Vec<_>>());
        println!("wifi {:?}", crate::net::wifi());
    }

    #[test]
    fn known_progids_get_friendly_names() {
        assert_eq!(app_name_for_progid("ChromeHTML"), "Google Chrome");
        assert_eq!(app_name_for_progid("MSEdgeHTM"), "Microsoft Edge");
        assert_eq!(app_name_for_progid("FirefoxURL-308046B0AF4A39CB"), "Firefox");
    }
}
