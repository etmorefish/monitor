//! Tiny two-language UI switch used by the egui shell.
//!
//! The terminal shell stays Chinese for now; the clickable globe/中 badge in
//! the GUI header toggles this global state, and every GUI label reads it.

use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn lang() -> Lang {
    if ENGLISH.load(Ordering::Relaxed) {
        Lang::En
    } else {
        Lang::Zh
    }
}

pub fn is_en() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

pub fn set(lang: Lang) {
    ENGLISH.store(lang == Lang::En, Ordering::Relaxed);
}

pub fn toggle() {
    ENGLISH.fetch_xor(true, Ordering::Relaxed);
}

/// Pick the Chinese or English variant of a static label.
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() {
        en
    } else {
        zh
    }
}

pub fn uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if is_en() {
        if days > 0 {
            format!("{days}d {hours:02}h {minutes:02}m")
        } else if hours > 0 {
            format!("{hours}h {minutes:02}m")
        } else {
            format!("{minutes}m")
        }
    } else if days > 0 {
        format!("{days}天{hours:02}时{minutes:02}分")
    } else if hours > 0 {
        format!("{hours}时{minutes:02}分")
    } else {
        format!("{minutes}分")
    }
}

pub fn age(seconds: u64) -> String {
    if is_en() {
        if seconds < 60 {
            "just now".to_string()
        } else if seconds < 3_600 {
            format!("{}m", seconds / 60)
        } else if seconds < 86_400 {
            format!("{}h {}m", seconds / 3_600, seconds % 3_600 / 60)
        } else {
            format!("{}d {}h", seconds / 86_400, seconds % 86_400 / 3_600)
        }
    } else if seconds < 60 {
        "刚刚".to_string()
    } else if seconds < 3_600 {
        format!("{}分", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}时{}分", seconds / 3_600, seconds % 3_600 / 60)
    } else {
        format!("{}天{}时", seconds / 86_400, seconds % 86_400 / 3_600)
    }
}

pub fn count(value: usize, zh_unit: &'static str, en_unit: &'static str) -> String {
    if is_en() {
        format!("{value} {en_unit}")
    } else {
        format!("{value} {zh_unit}")
    }
}
