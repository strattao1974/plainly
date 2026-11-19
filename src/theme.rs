//! Look and feel: Windows' own Segoe UI font and icon font, and a calm palette for light and dark mode.

use crate::journal::Kind;
use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle, Theme, Visuals};
use std::sync::Arc;

pub const SEMI: &str = "semibold";
pub const ICONS: &str = "icons";

pub fn semi() -> FontFamily {
    FontFamily::Name(SEMI.into())
}
pub fn icons() -> FontFamily {
    FontFamily::Name(ICONS.into())
}

pub mod icon {
    pub const TIMELINE: &str = "\u{E81C}";
    pub const SLOW: &str = "\u{E9D9}";
    pub const GLOBE: &str = "\u{E774}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const STARTUP: &str = "\u{E7E8}";
    pub const SERVICE: &str = "\u{E90F}";
    pub const TASK: &str = "\u{E787}";
    pub const APPS: &str = "\u{E71D}";
    pub const DEFAULTS: &str = "\u{E7AC}";
    pub const UPDATE: &str = "\u{E895}";
    pub const RESTART: &str = "\u{E72C}";
    pub const WARNING: &str = "\u{E7BA}";
    pub const SHIELD: &str = "\u{EA18}";
    pub const DISK: &str = "\u{EDA2}";
    pub const BOLT: &str = "\u{E945}";
    pub const INFO: &str = "\u{E946}";
    pub const CHECK: &str = "\u{E73E}";
    pub const CROSS: &str = "\u{E711}";
    pub const PC: &str = "\u{E7F4}";
    pub const ROUTER: &str = "\u{E968}";
    pub const LINK: &str = "\u{E71B}";
    pub const SEARCH: &str = "\u{E721}";
}

pub fn kind_icon(k: Kind) -> &'static str {
    match k {
        Kind::Startup => icon::STARTUP,
        Kind::Service => icon::SERVICE,
        Kind::Task => icon::TASK,
        Kind::Program => icon::APPS,
        Kind::DefaultApp => icon::DEFAULTS,
        Kind::Update => icon::UPDATE,
        Kind::Restart => icon::RESTART,
        Kind::Crash => icon::WARNING,
        Kind::Threat => icon::SHIELD,
        Kind::Disk => icon::DISK,
        Kind::Slowdown => icon::BOLT,
        Kind::Internet => icon::GLOBE,
        Kind::Info => icon::INFO,
    }
}

#[derive(Clone, Copy)]
pub struct Pal {
    pub dark: bool,
    pub bg: Color32,
    pub side: Color32,
    pub card: Color32,
    pub subtle: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub on_accent: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub bad: Color32,
    pub hover: Color32,
}

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            dark,
            bg: hex(0x16181c),
            side: hex(0x1c1f24),
            card: hex(0x22262c),
            subtle: hex(0x2a2f36),
            border: hex(0x323840),
            text: hex(0xe8eaed),
            weak: hex(0x9aa4b2),
            accent: hex(0x2dd4bf),
            accent_soft: Color32::from_rgba_unmultiplied(45, 212, 191, 38),
            on_accent: hex(0x0b1f1c),
            ok: hex(0x4ade80),
            warn: hex(0xfbbf24),
            bad: hex(0xf87171),
            hover: hex(0x262a31),
        }
    } else {
        Pal {
            dark,
            bg: hex(0xf6f7f9),
            side: hex(0xeceef2),
            card: hex(0xffffff),
            subtle: hex(0xf1f3f5),
            border: hex(0xe1e4e8),
            text: hex(0x1f2328),
            weak: hex(0x5f6b7a),
            accent: hex(0x0f766e),
            accent_soft: Color32::from_rgba_unmultiplied(15, 118, 110, 30),
            on_accent: hex(0xffffff),
            ok: hex(0x15803d),
            warn: hex(0xb45309),
            bad: hex(0xb91c1c),
            hover: hex(0xe3e6ea),
        }
    }
}

pub fn kind_color(k: Kind, dark: bool) -> Color32 {
    let (light, darkc) = match k {
        Kind::Startup | Kind::Service | Kind::Task | Kind::Program | Kind::DefaultApp => (0x2563eb, 0x60a5fa),
        Kind::Update | Kind::Restart => (0x7c3aed, 0xa78bfa),
        Kind::Crash | Kind::Threat | Kind::Disk => (0xdc2626, 0xf87171),
        Kind::Slowdown => (0xd97706, 0xfbbf24),
        Kind::Internet => (0x0891b2, 0x22d3ee),
        Kind::Info => (0x0f766e, 0x2dd4bf),
    };
    hex(if dark { darkc } else { light })
}

fn windows_font(name: &str) -> Option<Vec<u8>> {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    std::fs::read(format!(r"{dir}\Fonts\{name}")).ok()
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let default_prop = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();

    let mut prop = default_prop.clone();
    if let Some(b) = windows_font("segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(b)));
        prop.insert(0, "segoe".into());
    }
    let mut semi_list = prop.clone();
    if let Some(b) = windows_font("seguisb.ttf").or_else(|| windows_font("segoeuib.ttf")) {
        fonts.font_data.insert("segoe-semibold".into(), Arc::new(FontData::from_owned(b)));
        semi_list.insert(0, "segoe-semibold".into());
    }
    let mut icon_list = Vec::new();
    if let Some(b) = windows_font("SegoeIcons.ttf").or_else(|| windows_font("segmdl2.ttf")) {
        let mut fd = FontData::from_owned(b);
        fd.tweak.y_offset_factor = 0.08;
        fonts.font_data.insert("segoe-icons".into(), Arc::new(fd));
        icon_list.push("segoe-icons".to_string());
        prop.push("segoe-icons".into());
        semi_list.push("segoe-icons".into());
    }
    icon_list.extend(default_prop);
    fonts.families.insert(FontFamily::Proportional, prop);
    fonts.families.insert(semi(), semi_list);
    fonts.families.insert(icons(), icon_list);
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Heading, FontId::new(26.0, semi())),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(14.0, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(12.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
        ]
        .into();
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(12.0, 5.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.scroll.bar_width = 8.0;
    });
    ctx.set_visuals_of(Theme::Light, visuals(false));
    ctx.set_visuals_of(Theme::Dark, visuals(true));
    match std::env::var("PLAINLY_THEME").as_deref() {
        Ok("light") => ctx.set_theme(egui::ThemePreference::Light),
        Ok("dark") => ctx.set_theme(egui::ThemePreference::Dark),
        _ => {}
    }
}

fn visuals(dark: bool) -> Visuals {
    let p = pal(dark);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.window_corner_radius = 12.into();
    v.extreme_bg_color = p.card;
    v.faint_bg_color = p.subtle;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent_soft;
    v.selection.stroke = Stroke::new(1.0, p.accent);
    v.override_text_color = Some(p.text);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = 4.into();
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    v.widgets.inactive.weak_bg_fill = p.card;
    v.widgets.inactive.bg_fill = p.card;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.weak.gamma_multiply(0.7));
    v.widgets.hovered.weak_bg_fill = p.hover;
    v.widgets.hovered.bg_fill = p.hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.accent);
    v.widgets.active.weak_bg_fill = p.accent_soft;
    v.widgets.active.bg_fill = p.accent_soft;
    v
}
