use crate::ipc;
use crate::journal::{now, Action, Event, Journal, Kind};
use crate::net::{self, SiteResult, Speed, Step, StepState, Verdict};
use crate::perf::{Sampler, Totals};
use crate::settings::Settings;
use crate::shared::{read_status, Status};
use crate::slow::{self, Level, SlowReport};
use crate::theme::{self, icon, Pal};
use crate::update::{self, UpdateInfo};
use crate::winutil::{self, fmt_bytes};
use chrono::{Datelike, Local, TimeZone};
use eframe::egui::{
    self, Align, Align2, Button, Color32, CornerRadius, FontId, Frame, Layout, Margin, RichText, ScrollArea, Sense,
    Stroke, TextEdit, Ui, Vec2,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Timeline,
    Slow,
    Internet,
    Settings,
}

impl Page {
    pub fn from_name(s: &str) -> Page {
        match s {
            "slow" => Page::Slow,
            "internet" => Page::Internet,
            "settings" => Page::Settings,
            _ => Page::Timeline,
        }
    }
}

/// The window process's own state. The background watcher writes the timeline and status files;
/// the window reads them and owns settings.json.
struct UiState {
    dir: PathBuf,
    settings: Mutex<Settings>,
    journal: Mutex<Journal>,
    live: Mutex<Option<Totals>>,
    live_fast: AtomicBool,
    requested_page: Mutex<Option<Page>>,
    ctx: egui::Context,
    main_hwnd: AtomicIsize,
}

impl UiState {
    fn settings(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }
    fn save_settings(&self) {
        self.settings.lock().unwrap().save(&self.dir.join("settings.json"));
    }
    fn repaint(&self) {
        self.ctx.request_repaint();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    All,
    Changes,
    Updates,
    Problems,
    Slowdowns,
    Internet,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Filter::All => "Everything",
            Filter::Changes => "Changes",
            Filter::Updates => "Updates & restarts",
            Filter::Problems => "Problems",
            Filter::Slowdowns => "Slowdowns",
            Filter::Internet => "Internet",
        }
    }
    fn matches(self, k: Kind) -> bool {
        match self {
            Filter::All => true,
            Filter::Changes => matches!(k, Kind::Startup | Kind::Service | Kind::Task | Kind::Program | Kind::DefaultApp),
            Filter::Updates => matches!(k, Kind::Update | Kind::Restart),
            Filter::Problems => matches!(k, Kind::Crash | Kind::Threat | Kind::Disk),
            Filter::Slowdowns => k == Kind::Slowdown,
            Filter::Internet => k == Kind::Internet,
        }
    }
}

#[derive(Default)]
struct SlowJob {
    running: bool,
    report: Option<SlowReport>,
    at: i64,
}

#[derive(Default)]
struct NetJob {
    running: bool,
    steps: Vec<Step>,
    verdict: Option<Verdict>,
    speed_running: bool,
    speed: Option<Result<Speed, String>>,
    site_running: bool,
    site: Option<SiteResult>,
}

enum Confirm {
    EndTask { label: String, pids: Vec<u32> },
    ClearHistory,
}

#[derive(Default)]
struct UpdateJob {
    running: bool,
    result: Option<Result<UpdateInfo, String>>,
}

pub struct PlainlyApp {
    shared: Arc<UiState>,
    status: Status,
    agent_up: bool,
    status_checked: Option<Instant>,
    update: UpdateInfo,
    update_dismissed: bool,
    update_job: Arc<Mutex<UpdateJob>>,
    page: Page,
    filter: Filter,
    search: String,
    new_cutoff: i64,
    slow: Arc<Mutex<SlowJob>>,
    net: Arc<Mutex<NetJob>>,
    site_input: String,
    confirm: Option<Confirm>,
    flash: Option<(String, Instant)>,
    starts_with_windows: bool,
    welcome_autostart: bool,
}

impl PlainlyApp {
    pub fn new(cc: &eframe::CreationContext<'_>, dir: PathBuf, page: Option<String>) -> Self {
        theme::install(&cc.egui_ctx);
        let shared = Arc::new(UiState {
            settings: Mutex::new(Settings::load(&dir.join("settings.json"))),
            journal: Mutex::new(Journal::load(dir.join("timeline.jsonl"))),
            dir,
            live: Mutex::new(None),
            live_fast: AtomicBool::new(false),
            requested_page: Mutex::new(page.as_deref().map(Page::from_name)),
            ctx: cc.egui_ctx.clone(),
            main_hwnd: AtomicIsize::new(0),
        });
        if let Ok(h) = cc.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                shared.main_hwnd.store(w.hwnd.get(), Ordering::SeqCst);
            }
        }

        let s = shared.clone();
        ipc::listen_in_window(move |msg| {
            if let Some(p) = msg.strip_prefix("page:") {
                *s.requested_page.lock().unwrap() = Some(Page::from_name(p));
            }
            winutil::show_window(s.main_hwnd.load(Ordering::SeqCst));
            s.repaint();
        });

        // Live gauges for the "Why is it slow?" page; idle unless that page is showing.
        let s = shared.clone();
        std::thread::Builder::new()
            .name("live".into())
            .spawn(move || {
                let mut sampler = Sampler::new();
                loop {
                    if s.live_fast.load(Ordering::Relaxed) {
                        let t = sampler.sample(false).totals;
                        *s.live.lock().unwrap() = Some(t);
                        s.repaint();
                        std::thread::sleep(Duration::from_secs(1));
                    } else {
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            })
            .ok();

        let last_viewed = shared.settings().last_viewed;
        // When installed by the setup program, the installer already asked about starting with Windows.
        let installed = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("unins000.exe").exists()))
            .unwrap_or(false);
        let starts_with_windows = winutil::starts_with_windows();
        PlainlyApp {
            shared,
            status: Status::default(),
            agent_up: true,
            status_checked: None,
            update: UpdateInfo::default(),
            update_dismissed: false,
            update_job: Arc::default(),
            page: Page::Timeline,
            filter: Filter::All,
            search: String::new(),
            new_cutoff: last_viewed,
            slow: Arc::default(),
            net: Arc::default(),
            site_input: String::new(),
            confirm: None,
            flash: None,
            starts_with_windows,
            welcome_autostart: if installed { starts_with_windows } else { true },
        }
    }

    fn goto(&mut self, page: Page) {
        if self.page == Page::Timeline && page != Page::Timeline {
            self.mark_viewed();
        }
        if page == Page::Timeline && self.page != Page::Timeline {
            self.new_cutoff = self.shared.settings().last_viewed;
        }
        self.page = page;
    }

    fn mark_viewed(&self) {
        self.shared.settings.lock().unwrap().last_viewed = now();
        self.shared.save_settings();
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), Instant::now()));
    }

    fn start_slow(&self) {
        {
            let mut j = self.slow.lock().unwrap();
            if j.running {
                return;
            }
            j.running = true;
        }
        let job = self.slow.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let startup_count = crate::collect::startup()
                .values()
                .filter(|s| s.enabled && !s.name.to_ascii_lowercase().contains("plainly"))
                .count();
            let facts = slow::gather(startup_count);
            let report = slow::build(&facts);
            {
                let mut j = job.lock().unwrap();
                j.report = Some(report);
                j.running = false;
                j.at = now();
            }
            shared.repaint();
        });
    }

    fn start_net(&self) {
        {
            let mut j = self.net.lock().unwrap();
            if j.running {
                return;
            }
            j.running = true;
            j.verdict = None;
            j.steps = net::fresh_steps();
        }
        let job = self.net.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let (facts, steps) = net::run_check(|s| {
                job.lock().unwrap().steps = s.to_vec();
                shared.repaint();
            });
            let v = net::verdict(&facts);
            {
                let mut j = job.lock().unwrap();
                j.steps = steps;
                j.verdict = Some(v);
                j.running = false;
            }
            shared.repaint();
        });
    }

    fn start_speed(&self) {
        {
            let mut j = self.net.lock().unwrap();
            if j.speed_running {
                return;
            }
            j.speed_running = true;
            j.speed = None;
        }
        let job = self.net.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let r = net::speed_test();
            {
                let mut j = job.lock().unwrap();
                j.speed = Some(r);
                j.speed_running = false;
            }
            shared.repaint();
        });
    }

    fn start_site(&self) {
        let input = self.site_input.clone();
        {
            let mut j = self.net.lock().unwrap();
            if j.site_running {
                return;
            }
            j.site_running = true;
            j.site = None;
        }
        let job = self.net.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let r = net::check_site(&input);
            {
                let mut j = job.lock().unwrap();
                j.site = Some(r);
                j.site_running = false;
            }
            shared.repaint();
        });
    }

    fn run_action(&mut self, a: &Action) {
        match a {
            Action::Open { target, .. } => winutil::shell_open(target),
            Action::Run { exe, args, .. } => winutil::run(exe, args),
            Action::Reveal { path, .. } => winutil::reveal(path),
            Action::DisableStartup { hive, approved_key, name, .. } => {
                if winutil::set_startup_approved(hive, approved_key, name, false) {
                    self.say("Turned off. It won't start by itself any more (it's still installed).");
                } else {
                    self.say("Windows wouldn't let Plainly change that one, so here's the Startup apps page.");
                    winutil::shell_open("ms-settings:startupapps");
                }
                ipc::send_to_agent("scan");
            }
            Action::Page { page, .. } => {
                let p = Page::from_name(page);
                self.goto(p);
                if p == Page::Slow {
                    self.start_slow();
                } else if p == Page::Internet {
                    self.start_net();
                }
            }
        }
    }
}

impl eframe::App for PlainlyApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Closing really closes the window process; the background watcher keeps going in the tray.
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.page == Page::Timeline {
                self.mark_viewed();
            }
            if !self.shared.settings().told_about_tray && ipc::send_to_agent("hint") {
                self.shared.settings.lock().unwrap().told_about_tray = true;
                self.shared.save_settings();
            }
        }

        let stale = self.status_checked.map(|t| t.elapsed() > Duration::from_secs(3)).unwrap_or(true);
        if stale {
            self.status = read_status(&self.shared.dir);
            self.update = update::load(&self.shared.dir);
            self.agent_up = ipc::agent_running();
            self.status_checked = Some(Instant::now());
            self.shared.journal.lock().unwrap().reload_if_changed();
        }

        let requested = self.shared.requested_page.lock().unwrap().take();
        if let Some(p) = requested {
            self.goto(p);
            match p {
                Page::Slow => self.start_slow(),
                Page::Internet => self.start_net(),
                _ => {}
            }
        }

        let dark = ctx.style().visuals.dark_mode;
        let p = theme::pal(dark);
        self.shared.live_fast.store(self.page == Page::Slow, Ordering::Relaxed);

        self.sidebar(ctx, &p);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(p.bg).inner_margin(Margin { left: 32, right: 32, top: 26, bottom: 16 }))
            .show(ctx, |ui| {
                if let Some((msg, at)) = &self.flash {
                    if at.elapsed() < Duration::from_secs(5) {
                        let msg = msg.clone();
                        banner(ui, &p, &msg);
                        ui.add_space(10.0);
                    } else {
                        self.flash = None;
                    }
                }
                if self.update.available() && !self.update_dismissed {
                    let (mut download, mut later) = (false, false);
                    tinted_card(ui, &p, p.accent, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Plainly {} is available (you have {}).", self.update.latest, update::CURRENT))
                                    .family(theme::semi())
                                    .color(p.text),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if secondary(ui, &p, "Later").clicked() {
                                    later = true;
                                }
                                if primary(ui, &p, "Download").clicked() {
                                    download = true;
                                }
                            });
                        });
                    });
                    ui.add_space(10.0);
                    if download {
                        winutil::shell_open(&self.update.url);
                    }
                    if later {
                        self.update_dismissed = true;
                    }
                }
                if !self.agent_up {
                    let mut start = false;
                    tinted_card(ui, &p, p.warn, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Plainly isn't keeping watch right now, so new changes won't be noticed.")
                                    .color(p.text),
                            );
                            if secondary(ui, &p, "Start keeping watch").clicked() {
                                start = true;
                            }
                        });
                    });
                    ui.add_space(10.0);
                    if start {
                        ipc::start_agent();
                        self.status_checked = None;
                        self.say("Plainly is keeping watch again.");
                    }
                }
                match self.page {
                    Page::Timeline => self.timeline(ui, &p),
                    Page::Slow => self.slow_page(ui, &p),
                    Page::Internet => self.internet_page(ui, &p),
                    Page::Settings => self.settings_page(ui, &p),
                }
            });
        self.confirm_dialog(ctx, &p);

        let busy = self.slow.lock().unwrap().running || self.update_job.lock().unwrap().running || {
            let n = self.net.lock().unwrap();
            n.running || n.speed_running || n.site_running
        };
        if busy {
            ctx.request_repaint_after(Duration::from_millis(80));
        } else if self.page == Page::Slow {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else if self.flash.is_some() {
            ctx.request_repaint_after(Duration::from_millis(500));
        } else {
            ctx.request_repaint_after(Duration::from_secs(4));
        }
    }
}

// ---------- shared widgets ----------

fn card<R>(ui: &mut Ui, p: &Pal, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn tinted_card<R>(ui: &mut Ui, p: &Pal, tint: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let fill = blend(p.card, tint, if p.dark { 0.14 } else { 0.08 });
    Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, blend(p.border, tint, 0.45)))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(20, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

fn banner(ui: &mut Ui, p: &Pal, msg: &str) {
    Frame::new().fill(p.accent_soft).corner_radius(8).inner_margin(Margin::symmetric(14, 9)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::CHECK).family(theme::icons()).color(p.accent));
            ui.label(RichText::new(msg).color(p.text));
        });
    });
}

fn page_header(ui: &mut Ui, p: &Pal, title: &str, sub: &str) {
    ui.label(RichText::new(title).family(theme::semi()).size(26.0).color(p.text));
    ui.label(RichText::new(sub).size(14.5).color(p.weak));
    ui.add_space(16.0);
}

fn primary(ui: &mut Ui, p: &Pal, text: &str) -> egui::Response {
    ui.add(
        Button::new(RichText::new(text).family(theme::semi()).size(15.0).color(p.on_accent))
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(8)
            .min_size(egui::vec2(150.0, 38.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn secondary(ui: &mut Ui, p: &Pal, text: &str) -> egui::Response {
    ui.add(
        Button::new(RichText::new(text).size(13.5).color(p.text))
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(6)
            .min_size(egui::vec2(0.0, 28.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn chip(ui: &mut Ui, p: &Pal, text: &str, on: bool) -> bool {
    let (fill, stroke, color) = if on { (p.accent, p.accent, p.on_accent) } else { (p.card, p.border, p.text) };
    ui.add(
        Button::new(RichText::new(text).size(13.0).color(color))
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(14)
            .min_size(egui::vec2(0.0, 28.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .clicked()
}

fn section_title(ui: &mut Ui, p: &Pal, text: &str) {
    ui.add_space(14.0);
    ui.label(RichText::new(text).family(theme::semi()).size(16.0).color(p.text));
    ui.add_space(4.0);
}

fn icon_badge(ui: &mut Ui, glyph: &str, color: Color32, dark: bool, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    ui.painter().circle_filled(rect.center(), size / 2.0, color.gamma_multiply(if dark { 0.22 } else { 0.13 }));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::new(size * 0.46, theme::icons()), color);
}

fn meter(ui: &mut Ui, p: &Pal, frac: f32, color: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, p.subtle);
    let mut fill = rect;
    fill.set_width((w * frac.clamp(0.0, 1.0)).max(if frac > 0.0 { 4.0 } else { 0.0 }));
    ui.painter().rect_filled(fill, 3.0, color);
}

fn level_color(p: &Pal, pct: f32) -> Color32 {
    if pct >= 85.0 {
        p.bad
    } else if pct >= 60.0 {
        p.warn
    } else {
        p.ok
    }
}

fn clock(t: i64) -> String {
    Local.timestamp_opt(t, 0).single().map(|d| d.format("%-I:%M %p").to_string()).unwrap_or_default()
}

fn day_label(t: i64) -> String {
    let Some(d) = Local.timestamp_opt(t, 0).single() else { return String::new() };
    let today = Local::now().date_naive();
    let date = d.date_naive();
    if date == today {
        "Today".into()
    } else if Some(date) == today.pred_opt() {
        "Yesterday".into()
    } else if date.year() == today.year() {
        d.format("%A %-d %B").to_string()
    } else {
        d.format("%A %-d %B %Y").to_string()
    }
}

fn ago(t: i64) -> String {
    let s = now() - t;
    match s {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

// ---------- sidebar ----------

impl PlainlyApp {
    fn sidebar(&mut self, ctx: &egui::Context, p: &Pal) {
        let unseen = if self.page == Page::Timeline {
            0
        } else {
            let since = self.shared.settings().last_viewed;
            self.shared.journal.lock().unwrap().events().iter().filter(|e| e.time > since && !e.noise && e.kind != Kind::Info).count()
        };
        egui::SidePanel::left("nav")
            .exact_width(232.0)
            .resizable(false)
            .frame(Frame::new().fill(p.side).inner_margin(Margin { left: 14, right: 14, top: 22, bottom: 16 }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    logo(ui, 30.0);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Plainly").family(theme::semi()).size(20.0).color(p.text));
                        ui.label(RichText::new("Your PC, explained").size(12.0).color(p.weak));
                    });
                });
                ui.add_space(22.0);
                let items = [
                    (Page::Timeline, icon::TIMELINE, "Timeline", unseen),
                    (Page::Slow, icon::SLOW, "Why is it slow?", 0),
                    (Page::Internet, icon::GLOBE, "Internet check", 0),
                    (Page::Settings, icon::SETTINGS, "Settings", 0),
                ];
                for (page, glyph, label, badge) in items {
                    if nav_item(ui, p, glyph, label, self.page == page, badge) && self.page != page {
                        self.goto(page);
                        if page == Page::Slow && self.slow.lock().unwrap().report.is_none() {
                            self.start_slow();
                        }
                        if page == Page::Internet && self.net.lock().unwrap().steps.is_empty() {
                            self.start_net();
                        }
                    }
                    ui.add_space(2.0);
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    let last = self.status.last_scan;
                    let since = self.status.watching_since;
                    if secondary(ui, p, "Check for changes now").clicked() {
                        if ipc::send_to_agent("scan") {
                            self.say("Checking for changes…");
                            self.status_checked = None;
                        } else {
                            self.say("Plainly isn't running in the background, so it can't check right now.");
                        }
                    }
                    ui.add_space(4.0);
                    if last > 0 {
                        ui.label(RichText::new(format!("Last checked {}", ago(last))).size(12.0).color(p.weak));
                    } else {
                        ui.label(RichText::new("Taking a first look…").size(12.0).color(p.weak));
                    }
                    if since > 0 {
                        let d = Local.timestamp_opt(since, 0).single().map(|d| d.format("%-d %B").to_string()).unwrap_or_default();
                        ui.label(RichText::new(format!("Keeping watch since {d}")).size(12.0).color(p.weak));
                    }
                });
            });
    }
}

fn logo(ui: &mut Ui, size: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same((size * 0.22) as u8), Color32::from_rgb(0x13, 0x6c, 0x9a));
    let c = rect.center();
    painter.circle_stroke(c, size * 0.25, Stroke::new(size * 0.11, Color32::WHITE));
    painter.circle_filled(c, size * 0.10, Color32::WHITE);
}

fn nav_item(ui: &mut Ui, p: &Pal, glyph: &str, label: &str, selected: bool, badge: usize) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let bg = if selected {
        p.accent_soft
    } else if resp.hovered() {
        p.hover
    } else {
        Color32::TRANSPARENT
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, bg);
    if selected {
        let bar = egui::Rect::from_min_size(rect.left_top() + egui::vec2(0.0, 10.0), egui::vec2(3.0, 20.0));
        painter.rect_filled(bar, 2.0, p.accent);
    }
    let color = if selected { p.accent } else { p.weak };
    painter.text(rect.left_center() + egui::vec2(16.0, 0.0), Align2::LEFT_CENTER, glyph, FontId::new(16.0, theme::icons()), color);
    let font = if selected { FontId::new(14.5, theme::semi()) } else { FontId::new(14.5, egui::FontFamily::Proportional) };
    painter.text(rect.left_center() + egui::vec2(44.0, 0.0), Align2::LEFT_CENTER, label, font, p.text);
    if badge > 0 {
        let text = if badge > 99 { "99+".to_string() } else { badge.to_string() };
        let center = rect.right_center() - egui::vec2(20.0, 0.0);
        let w = 12.0 + 7.0 * text.len() as f32;
        let r = egui::Rect::from_center_size(center, egui::vec2(w.max(22.0), 20.0));
        painter.rect_filled(r, 10.0, p.accent);
        painter.text(center, Align2::CENTER_CENTER, text, FontId::new(12.0, theme::semi()), p.on_accent);
    }
    resp.clicked()
}

// ---------- timeline ----------

impl PlainlyApp {
    fn timeline(&mut self, ui: &mut Ui, p: &Pal) {
        page_header(ui, p, "Timeline", "What's changed on this PC, and when, in plain English.");
        let settings = self.shared.settings();

        ui.horizontal_wrapped(|ui| {
            for f in [Filter::All, Filter::Changes, Filter::Updates, Filter::Problems, Filter::Slowdowns, Filter::Internet] {
                if chip(ui, p, f.label(), self.filter == f) {
                    self.filter = f;
                }
            }
        });
        ui.add_space(4.0);
        let mut show_noise = settings.show_noise;
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.search)
                    .hint_text(format!("{}  Search the timeline", icon::SEARCH))
                    .desired_width(240.0)
                    .margin(Margin::symmetric(10, 5)),
            );
            ui.add_space(10.0);
            if ui
                .checkbox(&mut show_noise, RichText::new("Include Windows' own routine housekeeping").size(13.0).color(p.weak))
                .changed()
            {
                self.shared.settings.lock().unwrap().show_noise = show_noise;
                self.shared.save_settings();
            }
        });
        ui.add_space(10.0);

        let needle = self.search.trim().to_lowercase();
        let events: Vec<Event> = {
            let j = self.shared.journal.lock().unwrap();
            j.events()
                .iter()
                .rev()
                .filter(|e| show_noise || !e.noise)
                .filter(|e| self.filter.matches(e.kind))
                .filter(|e| {
                    needle.is_empty() || e.title.to_lowercase().contains(&needle) || e.detail.to_lowercase().contains(&needle)
                })
                .take(1500)
                .cloned()
                .collect()
        };

        let mut clicked: Option<Action> = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if !settings.welcomed {
                self.welcome(ui, p);
                ui.add_space(12.0);
            }
            if events.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(icon::TIMELINE).family(theme::icons()).size(36.0).color(p.weak));
                    ui.add_space(8.0);
                    let msg = if needle.is_empty() && self.filter == Filter::All {
                        "Nothing yet. Plainly is taking its first look; changes will appear here."
                    } else {
                        "Nothing matches that."
                    };
                    ui.label(RichText::new(msg).color(p.weak));
                });
                return;
            }
            let mut current_day = String::new();
            for e in &events {
                let day = day_label(e.time);
                if day != current_day {
                    ui.add_space(if current_day.is_empty() { 2.0 } else { 14.0 });
                    ui.label(RichText::new(&day).family(theme::semi()).size(13.5).color(p.weak));
                    ui.add_space(2.0);
                    current_day = day;
                }
                let is_new = e.time > self.new_cutoff && self.new_cutoff > 0 && !e.noise && e.kind != Kind::Info;
                if let Some(a) = event_card(ui, p, e, is_new) {
                    clicked = Some(a);
                }
                ui.add_space(6.0);
            }
            ui.add_space(20.0);
        });
        if let Some(a) = clicked {
            self.run_action(&a);
        }
    }

    fn welcome(&mut self, ui: &mut Ui, p: &Pal) {
        tinted_card(ui, p, p.accent, |ui| {
            ui.label(RichText::new("Welcome to Plainly").family(theme::semi()).size(18.0).color(p.text));
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Plainly keeps a plain-English diary of what happens to your PC behind your back: apps that add themselves to startup, new background services, installs, changed default apps, updates, restarts, crashes, slowdowns and internet drop-outs. Everything stays on this PC.",
                )
                .color(p.text),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("Closing the window keeps Plainly running by the clock, so it can keep watch. Right-click its icon there to quit.")
                    .color(p.weak),
            );
            ui.add_space(8.0);
            ui.checkbox(&mut self.welcome_autostart, "Start Plainly with Windows (recommended: it can only notice changes while it's running)");
            ui.add_space(8.0);
            if primary(ui, p, "Got it").clicked() {
                winutil::set_starts_with_windows(self.welcome_autostart);
                self.starts_with_windows = self.welcome_autostart;
                self.shared.settings.lock().unwrap().welcomed = true;
                self.shared.save_settings();
            }
        });
    }
}

fn event_card(ui: &mut Ui, p: &Pal, e: &Event, is_new: bool) -> Option<Action> {
    let mut clicked = None;
    let color = theme::kind_color(e.kind, p.dark);
    card(ui, p, |ui| {
        ui.horizontal_top(|ui| {
            icon_badge(ui, theme::kind_icon(e.kind), color, p.dark, 36.0);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(&e.title).family(theme::semi()).size(15.0).color(p.text));
                    if is_new {
                        ui.label(RichText::new(" NEW ").size(10.5).family(theme::semi()).color(p.on_accent).background_color(p.accent));
                    }
                });
                let when = if e.approx { "exact time unknown".to_string() } else { clock(e.time) };
                ui.label(RichText::new(format!("{} · {}", e.kind.label(), when)).size(12.0).color(p.weak));
                if !e.detail.is_empty() {
                    ui.add_space(2.0);
                    let lines: Vec<&str> = e.detail.lines().collect();
                    if lines.len() > 4 {
                        ui.label(RichText::new(lines[..3].join("\n")).color(p.weak));
                        egui::CollapsingHeader::new(RichText::new(format!("{} more", lines.len() - 3)).size(12.5).color(p.accent))
                            .id_salt(e.id)
                            .show(ui, |ui| {
                                ui.label(RichText::new(lines[3..].join("\n")).color(p.weak));
                            });
                    } else {
                        ui.label(RichText::new(&e.detail).color(p.weak));
                    }
                }
                if !e.actions.is_empty() {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        for a in &e.actions {
                            if secondary(ui, p, a.label()).clicked() {
                                clicked = Some(a.clone());
                            }
                        }
                    });
                }
            });
        });
    });
    clicked
}

// ---------- why is it slow ----------

impl PlainlyApp {
    fn slow_page(&mut self, ui: &mut Ui, p: &Pal) {
        page_header(ui, p, "Why is my PC slow?", "One click finds what's using your PC right now, and explains why.");
        let live = self.shared.live.lock().unwrap().clone();
        ui.columns(3, |cols| {
            let (cpu, mem, disk) = live.as_ref().map(|t| (t.cpu, t.mem_pct, t.disk)).unwrap_or((0.0, 0.0, None));
            let mem_text = live
                .as_ref()
                .map(|t| format!("{} of {}", fmt_bytes(t.mem_used), fmt_bytes(t.mem_total)))
                .unwrap_or_default();
            gauge(&mut cols[0], p, "Processor", cpu, "in use");
            gauge(&mut cols[1], p, "Memory", mem, &mem_text);
            match disk {
                Some(d) => gauge(&mut cols[2], p, "Disk", d, "busy"),
                None => gauge(&mut cols[2], p, "Disk", 0.0, "not available"),
            }
        });
        ui.add_space(14.0);

        let (running, report, at) = {
            let j = self.slow.lock().unwrap();
            (j.running, j.report.clone(), j.at)
        };
        ui.horizontal(|ui| {
            let label = if report.is_some() { "Check again" } else { "Check now" };
            if primary(ui, p, label).clicked() {
                self.start_slow();
            }
            if let Some(r) = &report {
                if secondary(ui, p, "Copy report").clicked() {
                    ui.ctx().copy_text(r.to_text());
                    self.say("Copied. Paste it into an email or a message to whoever helps you with your PC.");
                }
                ui.label(RichText::new(format!("Checked {}", ago(at))).size(12.0).color(p.weak));
            }
        });
        ui.add_space(10.0);

        let mut end_task: Option<(String, Vec<u32>)> = None;
        let mut action: Option<Action> = None;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if running {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Looking at everything that's running… (about 2 seconds)").color(p.weak));
                });
                return;
            }
            let Some(r) = report else { return };
            let tint = match r.level {
                Level::Calm => p.ok,
                Level::Busy => p.warn,
                Level::Strained => p.bad,
            };
            tinted_card(ui, p, tint, |ui| {
                ui.horizontal_top(|ui| {
                    let glyph = if r.level == Level::Calm { icon::CHECK } else { icon::WARNING };
                    icon_badge(ui, glyph, tint, p.dark, 42.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&r.headline).family(theme::semi()).size(19.0).color(p.text));
                        ui.add_space(2.0);
                        ui.label(RichText::new(&r.summary).color(p.text));
                    });
                });
            });

            if !r.culprits.is_empty() {
                let heading = match (r.level, r.resource) {
                    (Level::Calm, _) => "Using the most memory",
                    (_, slow::Resource::Cpu) => "What's using the processor",
                    (_, slow::Resource::Memory) => "What's using the memory",
                    (_, slow::Resource::Disk) => "What's using the disk",
                };
                section_title(ui, p, heading);
                for c in &r.culprits {
                    card(ui, p, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&c.label).family(theme::semi()).size(15.0).color(p.text));
                            if c.count > 1 {
                                ui.label(RichText::new(format!("({} parts)", c.count)).size(12.0).color(p.weak));
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "Processor {:.0}%   Memory {}   Disk {}/s",
                                        c.cpu,
                                        fmt_bytes(c.mem),
                                        fmt_bytes(c.io as u64)
                                    ))
                                    .size(12.5)
                                    .color(p.weak),
                                );
                            });
                        });
                        ui.label(RichText::new(&c.why).color(p.weak));
                        if let Some(tip) = &c.tip {
                            ui.label(RichText::new(format!("Tip: {tip}")).color(p.text));
                        }
                        if c.can_end {
                            ui.add_space(2.0);
                            if secondary(ui, p, "End task").clicked() {
                                end_task = Some((c.label.clone(), c.pids.clone()));
                            }
                        }
                    });
                    ui.add_space(6.0);
                }
            }

            if !r.findings.is_empty() {
                section_title(ui, p, "Other things that can slow your PC");
                for f in &r.findings {
                    card(ui, p, |ui| {
                        ui.horizontal_top(|ui| {
                            icon_badge(ui, icon::INFO, p.warn, p.dark, 30.0);
                            ui.add_space(4.0);
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&f.text).color(p.text));
                                if let Some(a) = &f.action {
                                    if secondary(ui, p, a.label()).clicked() {
                                        action = Some(a.clone());
                                    }
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
            }

            let recent: Vec<Event> = {
                let j = self.shared.journal.lock().unwrap();
                j.events()
                    .iter()
                    .rev()
                    .filter(|e| e.kind == Kind::Slowdown && e.time > now() - 7 * 86_400)
                    .take(5)
                    .cloned()
                    .collect()
            };
            section_title(ui, p, "Slowdowns in the last week");
            if recent.is_empty() {
                ui.label(RichText::new("None. Plainly hasn't caught your PC struggling for more than a minute.").color(p.weak));
            } else {
                for e in &recent {
                    card(ui, p, |ui| {
                        ui.label(RichText::new(format!("{} · {}", day_label(e.time), clock(e.time))).size(12.0).color(p.weak));
                        ui.label(RichText::new(&e.title).family(theme::semi()).color(p.text));
                        ui.label(RichText::new(&e.detail).color(p.weak));
                    });
                    ui.add_space(6.0);
                }
            }
            ui.add_space(20.0);
        });
        if let Some((label, pids)) = end_task {
            self.confirm = Some(Confirm::EndTask { label, pids });
        }
        if let Some(a) = action {
            self.run_action(&a);
        }
    }

    // ---------- internet ----------

    fn internet_page(&mut self, ui: &mut Ui, p: &Pal) {
        page_header(ui, p, "Is it me or the internet?", "Checks each link between your PC and the internet, and tells you which one is the problem.");
        let (running, steps, verdict, speed_running, speed, site_running, site) = {
            let j = self.net.lock().unwrap();
            (j.running, j.steps.clone(), j.verdict.clone(), j.speed_running, j.speed.clone(), j.site_running, j.site.clone())
        };
        ui.horizontal(|ui| {
            let label = if steps.is_empty() { "Run the check" } else { "Check again" };
            if ui.add_enabled_ui(!running, |ui| primary(ui, p, label)).inner.clicked() {
                self.start_net();
            }
            if let Some(v) = &verdict {
                if secondary(ui, p, "Copy report").clicked() {
                    ui.ctx().copy_text(net::report_text(&steps, v));
                    self.say("Copied. Handy if you need to contact your internet provider.");
                }
            }
        });
        ui.add_space(12.0);

        let mut run_speed = false;
        let mut run_site = false;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if !steps.is_empty() {
                card(ui, p, |ui| {
                    chain(ui, p, &steps);
                    ui.add_space(10.0);
                    for s in &steps {
                        step_row(ui, p, s);
                    }
                });
                ui.add_space(10.0);
            }
            if let Some(v) = &verdict {
                let tint = match v.who {
                    net::Who::AllGood => p.ok,
                    net::Who::Flaky | net::Who::SignIn => p.warn,
                    _ => p.bad,
                };
                tinted_card(ui, p, tint, |ui| {
                    ui.label(RichText::new(&v.headline).family(theme::semi()).size(19.0).color(p.text));
                    ui.add_space(2.0);
                    ui.label(RichText::new(&v.explanation).color(p.text));
                    if !v.tips.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new("What to try").family(theme::semi()).color(p.text));
                        for t in &v.tips {
                            ui.horizontal_top(|ui| {
                                ui.label(RichText::new("•").color(p.weak));
                                ui.label(RichText::new(t).color(p.text));
                            });
                        }
                    }
                });
            }

            section_title(ui, p, "Speed");
            card(ui, p, |ui| {
                ui.label(RichText::new("Downloads a test file for up to 10 seconds to measure your speed, and checks whether the connection gets laggy while busy.").color(p.weak));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled_ui(!speed_running, |ui| secondary(ui, p, "Test speed")).inner.clicked() {
                        run_speed = true;
                    }
                    if speed_running {
                        ui.spinner();
                        ui.label(RichText::new("Measuring…").color(p.weak));
                    }
                });
                match &speed {
                    Some(Ok(s)) => {
                        let (a, b) = s.describe();
                        ui.add_space(6.0);
                        ui.label(RichText::new(a).family(theme::semi()).color(p.text));
                        if !b.is_empty() {
                            ui.label(RichText::new(b).color(p.weak));
                        }
                    }
                    Some(Err(e)) => {
                        ui.label(RichText::new(format!("The speed test didn't work: {e}.")).color(p.bad));
                    }
                    None => {}
                }
            });

            section_title(ui, p, "Is one website not working?");
            card(ui, p, |ui| {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        TextEdit::singleline(&mut self.site_input)
                            .hint_text("Type its address, e.g. bbc.co.uk")
                            .desired_width(300.0)
                            .margin(Margin::symmetric(10, 6)),
                    );
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.add_enabled_ui(!site_running, |ui| secondary(ui, p, "Check it")).inner.clicked() || enter {
                        run_site = true;
                    }
                    if site_running {
                        ui.spinner();
                    }
                });
                if let Some(s) = &site {
                    ui.add_space(6.0);
                    ui.horizontal_top(|ui| {
                        let (g, c) = if s.ok { (icon::CHECK, p.ok) } else { (icon::WARNING, p.warn) };
                        ui.label(RichText::new(g).family(theme::icons()).color(c));
                        ui.label(RichText::new(&s.text).color(p.text));
                    });
                }
            });

            let drops: Vec<Event> = {
                let j = self.shared.journal.lock().unwrap();
                j.events()
                    .iter()
                    .rev()
                    .filter(|e| e.kind == Kind::Internet && !e.noise && e.time > now() - 7 * 86_400)
                    .take(8)
                    .cloned()
                    .collect()
            };
            section_title(ui, p, "Drop-outs in the last week");
            if drops.is_empty() {
                ui.label(RichText::new("None. Plainly checks every 30 seconds and hasn't seen your internet drop out.").color(p.weak));
            } else {
                for e in &drops {
                    card(ui, p, |ui| {
                        ui.label(RichText::new(format!("{} · {}", day_label(e.time), clock(e.time))).size(12.0).color(p.weak));
                        ui.label(RichText::new(&e.title).family(theme::semi()).color(p.text));
                        ui.label(RichText::new(&e.detail).color(p.weak));
                    });
                    ui.add_space(6.0);
                }
            }
            ui.add_space(20.0);
        });
        if run_speed {
            self.start_speed();
        }
        if run_site {
            self.start_site();
        }
    }

    // ---------- settings ----------

    fn settings_page(&mut self, ui: &mut Ui, p: &Pal) {
        page_header(ui, p, "Settings", "Plainly works out of the box. Adjust it here if you like.");
        let mut s = self.shared.settings();
        let before = serde_json::to_string(&s).unwrap_or_default();
        let mut clear = false;
        let mut check_updates_now = false;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section_title(ui, p, "Running in the background");
            card(ui, p, |ui| {
                let mut sw = self.starts_with_windows;
                if ui.checkbox(&mut sw, "Start Plainly with Windows").changed() {
                    winutil::set_starts_with_windows(sw);
                    self.starts_with_windows = sw;
                }
                ui.label(RichText::new("Plainly can only notice changes while it's running. It uses very little memory or processor.").size(12.5).color(p.weak));
                ui.add_space(6.0);
                ui.checkbox(&mut s.watch_slowdowns, "Watch for slowdowns (checks load every 5 seconds)");
                ui.checkbox(&mut s.watch_internet, "Watch the internet connection (checks every 30 seconds)");
            });

            section_title(ui, p, "Tell me when…");
            card(ui, p, |ui| {
                ui.checkbox(&mut s.notify_startup, "An app adds itself to startup");
                ui.checkbox(&mut s.notify_services, "A new background service or scheduled task appears");
                ui.checkbox(&mut s.notify_defaults, "A default app changes (browser, PDF, email…)");
                ui.checkbox(&mut s.notify_threats, "Microsoft Defender finds a threat");
                ui.checkbox(&mut s.notify_installs, "Something is installed");
                ui.checkbox(&mut s.notify_internet, "The internet comes back after dropping out");
                ui.checkbox(&mut s.notify_slowdowns, "The PC was struggling for a while");
                ui.label(RichText::new("Notifications respect Windows' Do not disturb.").size(12.5).color(p.weak));
            });

            section_title(ui, p, "Timeline");
            card(ui, p, |ui| {
                ui.checkbox(&mut s.show_noise, "Include Windows' own routine housekeeping");
                ui.label(RichText::new("Windows constantly updates its own services, tasks and Store apps. Plainly hides that so the important things stand out.").size(12.5).color(p.weak));
                ui.add_space(6.0);
                if secondary(ui, p, "Clear history…").clicked() {
                    clear = true;
                }
            });

            section_title(ui, p, "Your privacy");
            card(ui, p, |ui| {
                ui.label(RichText::new("Everything Plainly records stays on this PC. It has no account, no ads and no tracking.").color(p.text));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Apart from the update check, the only things it contacts are for the internet checks: your router, 1.1.1.1, 8.8.8.8 and 9.9.9.9 (to test the connection), www.msftconnecttest.com (Windows' own connection test), www.cloudflare.com, and speed.cloudflare.com (only when you run a speed test). Once a day it asks api.github.com whether there's a newer version, unless you turn that off below.",
                    )
                    .size(12.5)
                    .color(p.weak),
                );
                ui.add_space(6.0);
                if secondary(ui, p, "Open Plainly's data folder").clicked() {
                    winutil::shell_open(&self.shared.dir.display().to_string());
                }
            });

            section_title(ui, p, "About");
            card(ui, p, |ui| {
                ui.label(RichText::new(format!("Plainly {}", update::CURRENT)).family(theme::semi()).color(p.text));
                ui.label(RichText::new("Free and open source (MIT licence). Made with Rust.").color(p.weak));
                ui.add_space(6.0);
                ui.checkbox(&mut s.check_updates, "Check for updates once a day");
                ui.label(
                    RichText::new("Plainly only tells you about a new version and links to its download page. It never installs anything by itself.")
                        .size(12.5)
                        .color(p.weak),
                );
                ui.add_space(4.0);
                let (running, result) = {
                    let j = self.update_job.lock().unwrap();
                    (j.running, j.result.clone())
                };
                ui.horizontal(|ui| {
                    if ui.add_enabled_ui(!running, |ui| secondary(ui, p, "Check for updates now")).inner.clicked() {
                        check_updates_now = true;
                    }
                    if running {
                        ui.spinner();
                    }
                    match &result {
                        Some(Ok(info)) if info.available() => {
                            ui.label(RichText::new(format!("Version {} is available.", info.latest)).color(p.text));
                            if ui.link("Download it").clicked() {
                                winutil::shell_open(&info.url);
                            }
                        }
                        Some(Ok(_)) => {
                            ui.label(RichText::new("You have the latest version.").color(p.weak));
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(format!("Couldn't check: {e}.")).color(p.weak));
                        }
                        None => {}
                    }
                });
            });
            ui.add_space(20.0);
        });
        if serde_json::to_string(&s).unwrap_or_default() != before {
            *self.shared.settings.lock().unwrap() = s;
            self.shared.save_settings();
        }
        if clear {
            self.confirm = Some(Confirm::ClearHistory);
        }
        if check_updates_now {
            self.start_update_check();
        }
    }

    fn start_update_check(&self) {
        {
            let mut j = self.update_job.lock().unwrap();
            if j.running {
                return;
            }
            j.running = true;
            j.result = None;
        }
        let job = self.update_job.clone();
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let r = update::check_now(&shared.dir);
            {
                let mut j = job.lock().unwrap();
                j.result = Some(r);
                j.running = false;
            }
            shared.repaint();
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context, p: &Pal) {
        let Some(c) = &self.confirm else { return };
        let (title, body, yes) = match c {
            Confirm::EndTask { label, .. } => (
                format!("End {label}?"),
                "It will close straight away. Anything unsaved in it will be lost.".to_string(),
                "End task",
            ),
            Confirm::ClearHistory => (
                "Clear the timeline?".to_string(),
                "This deletes everything Plainly has recorded. It will keep watching from now on.".to_string(),
                "Clear history",
            ),
        };
        let mut decision: Option<bool> = None;
        egui::Area::new(egui::Id::new("dim")).order(egui::Order::Middle).fixed_pos(egui::Pos2::ZERO).show(ctx, |ui| {
            let screen = ctx.screen_rect();
            ui.painter().rect_filled(screen, 0.0, Color32::from_black_alpha(110));
            ui.allocate_rect(screen, Sense::click());
        });
        egui::Window::new("confirm")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .order(egui::Order::Foreground)
            .frame(Frame::new().fill(p.card).stroke(Stroke::new(1.0, p.border)).corner_radius(12).inner_margin(Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(&title).family(theme::semi()).size(18.0).color(p.text));
                ui.add_space(4.0);
                ui.label(RichText::new(&body).color(p.weak));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if primary(ui, p, yes).clicked() {
                        decision = Some(true);
                    }
                    if secondary(ui, p, "Cancel").clicked() {
                        decision = Some(false);
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            decision = Some(false);
        }
        match decision {
            Some(true) => {
                match self.confirm.take() {
                    Some(Confirm::EndTask { label, pids }) => {
                        let ended = pids.iter().filter(|&&pid| winutil::terminate(pid)).count();
                        if ended > 0 {
                            self.say(format!("Ended {label}."));
                        } else {
                            self.say(format!("Windows wouldn't let Plainly end {label}. Try Task Manager."));
                        }
                        self.slow.lock().unwrap().report = None;
                        self.start_slow();
                    }
                    Some(Confirm::ClearHistory) => {
                        self.shared.journal.lock().unwrap().clear();
                        self.say("Timeline cleared.");
                    }
                    None => {}
                }
            }
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}

fn gauge(ui: &mut Ui, p: &Pal, label: &str, pct: f32, sub: &str) {
    card(ui, p, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(p.weak));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("{pct:.0}%")).family(theme::semi()).size(20.0).color(p.text));
            });
        });
        meter(ui, p, pct / 100.0, level_color(p, pct));
        ui.label(RichText::new(sub).size(12.0).color(p.weak));
    });
}

fn state_color(p: &Pal, s: StepState) -> Color32 {
    match s {
        StepState::Ok => p.ok,
        StepState::Warn => p.warn,
        StepState::Fail => p.bad,
        StepState::Running => p.accent,
        StepState::Waiting | StepState::Skipped => p.weak,
    }
}

fn chain(ui: &mut Ui, p: &Pal, steps: &[Step]) {
    let glyphs = [icon::PC, icon::ROUTER, icon::GLOBE, icon::LINK, icon::CHECK];
    let labels = ["Your PC", "Router", "Internet", "Address book", "Websites"];
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, 92.0), Sense::hover());
    let painter = ui.painter();
    let n = steps.len().min(5);
    if n == 0 {
        return;
    }
    let pad = 46.0;
    let xs: Vec<f32> = (0..n).map(|i| rect.left() + pad + i as f32 * (w - 2.0 * pad) / (n as f32 - 1.0).max(1.0)).collect();
    let y = rect.top() + 32.0;
    let t = ui.input(|i| i.time) as f32;
    for i in 1..n {
        let st = steps[i].state;
        let color = match st {
            StepState::Waiting | StepState::Skipped | StepState::Running => p.border,
            _ => state_color(p, st),
        };
        painter.line_segment([egui::pos2(xs[i - 1] + 24.0, y), egui::pos2(xs[i] - 24.0, y)], Stroke::new(3.0, color));
    }
    for i in 0..n {
        let st = steps[i].state;
        let c = state_color(p, st);
        let center = egui::pos2(xs[i], y);
        let pulse = if st == StepState::Running { 0.25 + 0.15 * (t * 4.0).sin() } else { 0.18 };
        painter.circle_filled(center, 23.0, c.gamma_multiply(if st == StepState::Waiting { 0.08 } else { pulse }));
        painter.circle_stroke(center, 23.0, Stroke::new(1.5, c.gamma_multiply(0.7)));
        let glyph = match st {
            StepState::Fail => icon::CROSS,
            _ => glyphs[i],
        };
        painter.text(center, Align2::CENTER_CENTER, glyph, FontId::new(18.0, theme::icons()), c);
        painter.text(egui::pos2(xs[i], y + 36.0), Align2::CENTER_CENTER, labels[i], FontId::new(12.5, egui::FontFamily::Proportional), p.text);
    }
}

fn step_row(ui: &mut Ui, p: &Pal, s: &Step) {
    ui.horizontal_top(|ui| {
        let c = state_color(p, s.state);
        match s.state {
            StepState::Running => {
                ui.add(egui::Spinner::new().size(16.0).color(c));
            }
            _ => {
                let g = match s.state {
                    StepState::Ok => icon::CHECK,
                    StepState::Warn => icon::WARNING,
                    StepState::Fail => icon::CROSS,
                    _ => "\u{2022}",
                };
                ui.label(RichText::new(g).family(theme::icons()).size(15.0).color(c));
            }
        }
        ui.label(RichText::new(s.title).family(theme::semi()).color(p.text));
        let detail = match s.state {
            StepState::Waiting => "Waiting…".to_string(),
            StepState::Running => "Checking…".to_string(),
            StepState::Skipped => "Skipped (an earlier link is broken).".to_string(),
            _ => s.detail.clone(),
        };
        ui.label(RichText::new(detail).color(p.weak));
    });
}
