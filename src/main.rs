#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::{Datelike, Duration as ChronoDuration, Local, TimeZone, Timelike};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use std::{fs, path::PathBuf, sync::Mutex};
use std::process::Command as ProcessCommand;
use base64::Engine;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindowBuilder,
};

const HOURS: usize = 24;
const WINDOW_24H_SECS: i64 = 24 * 60 * 60;
const RETENTION_SECS: i64 = 180 * WINDOW_24H_SECS;
const MIN_EXPORT_RECORDS: u32 = 5;
const REMINDER_WIDTH: i32 = 640;
const REMINDER_HEIGHT: i32 = 196;
const REMINDER_PROMPT_COUNT: usize = 15;
const DEFAULT_INTERVAL_MINUTES: u64 = 50;
const ALLOWED_INTERVAL_MINUTES: [u64; 5] = [5, 10, 20, 30, 50];
const TRAY_ID: &str = "main_tray";
const REMINDER_TIPS_EN: [&str; REMINDER_PROMPT_COUNT] = [
    "Smelly butt, smelly butt, please stand up!",
    "Your chakras are literally flattening. Stand up!",
    "The chair is NOT your lobster. Move!",
    "My spirit says your butt needs freedom!",
    "Could you BE sitting any longer?",
    "Could your butt BE any flatter? Stand!",
    "Could this chair BE more attached to you?",
    "So, I'm just gonna DIE here sitting?",
    "Could sitting here BE any sadder? Move!",
    "Your posture is a MESS. Stand up.",
    "If you won't move, I'll MAKE you move!",
    "How YOU sittin'? Get up already!",
    "Stand up or your sandwich gets it!",
    "Oh. My. God. You're STILL sitting?!",
    "Nooo, you can't sit forever. It's like... so bad!",
];

#[derive(Clone, Serialize, Deserialize)]
struct ReminderRecord {
    ts: i64,
    duration_secs: u64,
}

#[derive(Serialize, Deserialize)]
struct AppConfigFile {
    interval_minutes: u64,
    #[serde(default = "default_language")]
    language: String,
    #[serde(default = "default_reminder_language")]
    reminder_language: String,
    #[serde(default = "default_theme")]
    theme: String,
}

fn default_language() -> String {
    "en".to_string()
}

fn default_reminder_language() -> String {
    "en".to_string()
}

fn default_theme() -> String {
    "night".to_string()
}

fn sanitize_interval_minutes(value: u64) -> u64 {
    if ALLOWED_INTERVAL_MINUTES.contains(&value) {
        value
    } else {
        DEFAULT_INTERVAL_MINUTES
    }
}

#[derive(Serialize, Deserialize)]
struct AnalyticsStore {
    reminder_events: Vec<ReminderRecord>,
    standup_events: Vec<i64>,
}

#[derive(Serialize, Deserialize)]
struct AnalyticsData {
    hourly_sedentary: Vec<u32>,
    hourly_standup: Vec<u32>,
    hourly_sedentary_delay_secs: Vec<u64>,
    standup_sessions: u32,
    sedentary_sessions: u32,
    total_sitting_secs: u64,
    record_count: u32,
}

#[derive(Clone, Serialize)]
struct ActiveReminderPayload {
    id: u64,
    text: String,
    theme: String,
    visible: bool,
}

struct AppState {
    interval: Mutex<u64>,
    elapsed: Mutex<u64>,
    last_interval_change: Mutex<Instant>,
    reminder_events: Mutex<Vec<ReminderRecord>>,
    standup_events: Mutex<Vec<i64>>,
    reminder_visible: Mutex<bool>,
    language: Mutex<String>,
    reminder_language: Mutex<String>,
    theme: Mutex<String>,
    last_tip_index: Mutex<Option<usize>>,
    active_reminder_id: Mutex<u64>,
    active_reminder_start_ts: Mutex<Option<i64>>,
    active_reminder_shown_at: Mutex<Option<Instant>>,
    active_reminder_interval_secs: Mutex<u64>,
    active_reminder_logged_sedentary: Mutex<bool>,
    active_reminder_tip: Mutex<String>,
}

fn now_ts() -> i64 {
    Local::now().timestamp()
}

fn prune_old_events(reminders: &mut Vec<ReminderRecord>, standups: &mut Vec<i64>, now: i64) {
    let cutoff = now - RETENTION_SECS;
    reminders.retain(|r| r.ts >= cutoff);
    standups.retain(|ts| *ts >= cutoff);
}

fn normalize_period(period: &str) -> &'static str {
    match period {
        "weekly" => "weekly",
        "monthly" => "monthly",
        _ => "daily",
    }
}

fn local_midnight_ts(date: chrono::NaiveDate) -> i64 {
    let Some(naive) = date.and_hms_opt(0, 0, 0) else {
        return Local::now().timestamp();
    };
    Local
        .from_local_datetime(&naive)
        .single()
        .or_else(|| Local.from_local_datetime(&naive).earliest())
        .or_else(|| Local.from_local_datetime(&naive).latest())
        .map(|dt| dt.timestamp())
        .unwrap_or_else(|| Local::now().timestamp())
}

fn period_start_ts(period: &str, now: chrono::DateTime<Local>) -> i64 {
    let p = normalize_period(period);
    match p {
        "weekly" => local_midnight_ts(now.date_naive() - ChronoDuration::days(6)),
        "monthly" => {
            let first = chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
                .unwrap_or_else(|| now.date_naive());
            local_midnight_ts(first)
        }
        _ => local_midnight_ts(now.date_naive()),
    }
}

fn config_path(handle: &AppHandle) -> Option<PathBuf> {
    handle
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("config.json"))
}

fn analytics_path(handle: &AppHandle) -> Option<PathBuf> {
    handle
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("analytics.json"))
}

fn legacy_app_data_dir(handle: &AppHandle) -> Option<PathBuf> {
    let current = handle.path().app_data_dir().ok()?;
    let parent = current.parent()?;
    let legacy = parent.join("com.colinwhispers.standby");
    legacy.exists().then_some(legacy)
}

fn export_dir(handle: &AppHandle) -> Option<PathBuf> {
    handle
        .path()
        .download_dir()
        .ok()
        .or_else(|| handle.path().desktop_dir().ok())
        .or_else(|| handle.path().app_data_dir().ok())
}

fn read_config(handle: &AppHandle) -> AppConfigFile {
    if let Some(path) = config_path(handle) {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfigFile>(&contents) {
                return cfg;
            }
        }
    }
    if let Some(path) = legacy_app_data_dir(handle).map(|dir| dir.join("config.json")) {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfigFile>(&contents) {
                return cfg;
            }
        }
    }
    AppConfigFile {
        interval_minutes: DEFAULT_INTERVAL_MINUTES,
        language: default_language(),
        reminder_language: default_reminder_language(),
        theme: default_theme(),
    }
}

fn save_config(
    handle: &AppHandle,
    minutes: u64,
    language: &str,
    reminder_language: &str,
    theme: &str,
) {
    if let Some(path) = config_path(handle) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let cfg = AppConfigFile {
            interval_minutes: minutes,
            language: language.to_string(),
            reminder_language: reminder_language.to_string(),
            theme: theme.to_string(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&cfg) {
            let _ = fs::write(path, json);
        }
    }
}

fn load_config(handle: &AppHandle, state: &AppState) {
    let cfg = read_config(handle);
    let normalized_minutes = sanitize_interval_minutes(cfg.interval_minutes);
    let normalized_language = if cfg.language == "zh-CN" {
        "zh-CN".to_string()
    } else {
        "en".to_string()
    };
    let normalized_reminder_language = if cfg.reminder_language == "zh-CN" {
        "zh-CN".to_string()
    } else {
        "en".to_string()
    };
    let normalized_theme = if cfg.theme == "day" {
        "day".to_string()
    } else {
        "night".to_string()
    };

    *state.interval.lock().unwrap() = normalized_minutes * 60;
    *state.language.lock().unwrap() = normalized_language.clone();
    *state.reminder_language.lock().unwrap() = normalized_reminder_language.clone();
    *state.theme.lock().unwrap() = normalized_theme.clone();

    // Persist normalized/migrated config into the current app data path.
    save_config(
        handle,
        normalized_minutes,
        &normalized_language,
        &normalized_reminder_language,
        &normalized_theme,
    );
}

fn tray_label(lang: &str, en: &str, zh: &str) -> String {
    if lang == "zh-CN" {
        zh.to_string()
    } else {
        en.to_string()
    }
}

fn make_tray_menu(app: &AppHandle, lang: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let open_settings = MenuItem::with_id(
        app,
        "open_settings",
        tray_label(lang, "Open Settings", "打开设置"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        tray_label(lang, "Quit", "退出"),
        true,
        None::<&str>,
    )?;
    Menu::with_items(app, &[&open_settings, &quit])
}

fn refresh_tray_menu(app: &AppHandle, lang: &str) {
    if let (Some(tray), Ok(menu)) = (app.tray_by_id(TRAY_ID), make_tray_menu(app, lang)) {
        let _ = tray.set_menu(Some(menu));
    }
}

fn save_analytics(handle: &AppHandle, state: &AppState) {
    if let Some(path) = analytics_path(handle) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let now = now_ts();
        let mut reminders = state.reminder_events.lock().unwrap().clone();
        let mut standups = state.standup_events.lock().unwrap().clone();
        prune_old_events(&mut reminders, &mut standups, now);

        let store = AnalyticsStore {
            reminder_events: reminders,
            standup_events: standups,
        };

        if let Ok(json) = serde_json::to_string_pretty(&store) {
            let _ = fs::write(path, json);
        }
    }
}

fn load_analytics(handle: &AppHandle, state: &AppState) {
    if let Some(path) = analytics_path(handle) {
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(mut data) = serde_json::from_str::<AnalyticsStore>(&contents) {
                let now = now_ts();
                prune_old_events(&mut data.reminder_events, &mut data.standup_events, now);
                *state.reminder_events.lock().unwrap() = data.reminder_events;
                *state.standup_events.lock().unwrap() = data.standup_events;
                return;
            }
        }
    }
    if let Some(path) = legacy_app_data_dir(handle).map(|dir| dir.join("analytics.json")) {
        if let Ok(contents) = fs::read_to_string(path) {
            if let Ok(mut data) = serde_json::from_str::<AnalyticsStore>(&contents) {
                let now = now_ts();
                prune_old_events(&mut data.reminder_events, &mut data.standup_events, now);
                *state.reminder_events.lock().unwrap() = data.reminder_events;
                *state.standup_events.lock().unwrap() = data.standup_events;
            }
        }
    }
}

fn build_analytics_for_period(state: &AppState, period: &str) -> AnalyticsData {
    let now = now_ts();
    let mut reminders = state.reminder_events.lock().unwrap();
    let mut standups = state.standup_events.lock().unwrap();
    prune_old_events(&mut reminders, &mut standups, now);
    let start_ts = period_start_ts(period, Local::now());

    let mut hourly_sedentary = vec![0u32; HOURS];
    let mut hourly_standup = vec![0u32; HOURS];
    let mut hourly_sedentary_delay_secs = vec![0u64; HOURS];

    let filtered_reminders: Vec<ReminderRecord> = reminders
        .iter()
        .filter(|e| e.ts >= start_ts)
        .cloned()
        .collect();
    let filtered_standups: Vec<i64> = standups.iter().copied().filter(|ts| *ts >= start_ts).collect();

    for event in filtered_reminders.iter() {
        if let Some(dt) = Local.timestamp_opt(event.ts, 0).single() {
            hourly_sedentary[dt.hour() as usize] += 1;
            hourly_sedentary_delay_secs[dt.hour() as usize] += event.duration_secs;
        }
    }

    for ts in filtered_standups.iter() {
        if let Some(dt) = Local.timestamp_opt(*ts, 0).single() {
            hourly_standup[dt.hour() as usize] += 1;
        }
    }

    let total_sitting_secs = filtered_reminders.iter().map(|e| e.duration_secs).sum::<u64>();
    let sedentary_sessions = filtered_reminders.len() as u32;
    let standup_sessions = filtered_standups.len() as u32;

    AnalyticsData {
        hourly_sedentary,
        hourly_standup,
        hourly_sedentary_delay_secs,
        standup_sessions,
        sedentary_sessions,
        total_sitting_secs,
        record_count: sedentary_sessions + standup_sessions,
    }
}

fn build_analytics(state: &AppState) -> AnalyticsData {
    build_analytics_for_period(state, "daily")
}

#[tauri::command]
fn set_reminder_interval(app: AppHandle, minutes: u64, state: State<'_, AppState>) -> String {
    let normalized_minutes = sanitize_interval_minutes(minutes);
    let mut interval = state.interval.lock().unwrap();
    *interval = normalized_minutes * 60;

    let mut elapsed = state.elapsed.lock().unwrap();
    *elapsed = 0;

    let mut last_change = state.last_interval_change.lock().unwrap();
    *last_change = Instant::now();

    let language = state.language.lock().unwrap().clone();
    let reminder_language = state.reminder_language.lock().unwrap().clone();
    let theme = state.theme.lock().unwrap().clone();
    save_config(
        &app,
        normalized_minutes,
        &language,
        &reminder_language,
        &theme,
    );
    format!("Interval set to {} minutes", normalized_minutes)
}

#[tauri::command]
fn get_reminder_interval(state: State<'_, AppState>) -> u64 {
    (*state.interval.lock().unwrap()) / 60
}

#[tauri::command]
fn set_language(app: AppHandle, language: String, state: State<'_, AppState>) -> Result<(), String> {
    let normalized = match language.as_str() {
        "zh-CN" => "zh-CN".to_string(),
        _ => "en".to_string(),
    };

    {
        let mut lang = state.language.lock().unwrap();
        *lang = normalized.clone();
    }

    let minutes = (*state.interval.lock().unwrap()) / 60;
    let reminder_language = state.reminder_language.lock().unwrap().clone();
    let theme = state.theme.lock().unwrap().clone();
    save_config(&app, minutes, &normalized, &reminder_language, &theme);
    refresh_tray_menu(&app, &normalized);
    let _ = app.emit("language-changed", normalized);
    Ok(())
}

#[tauri::command]
fn get_language(state: State<'_, AppState>) -> String {
    state.language.lock().unwrap().clone()
}

#[tauri::command]
fn set_reminder_language(
    app: AppHandle,
    language: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let normalized = match language.as_str() {
        "zh-CN" => "zh-CN".to_string(),
        _ => "en".to_string(),
    };
    {
        let mut lang = state.reminder_language.lock().unwrap();
        *lang = normalized.clone();
    }

    let minutes = (*state.interval.lock().unwrap()) / 60;
    let ui_language = state.language.lock().unwrap().clone();
    let theme = state.theme.lock().unwrap().clone();
    save_config(&app, minutes, &ui_language, &normalized, &theme);
    let _ = app.emit("reminder-language-changed", normalized);
    Ok(())
}

#[tauri::command]
fn get_reminder_language(state: State<'_, AppState>) -> String {
    state.reminder_language.lock().unwrap().clone()
}

#[tauri::command]
fn next_reminder_tip_index(state: State<'_, AppState>) -> u32 {
    next_tip_index_from_state(&state) as u32
}

fn next_tip_index_from_state(state: &AppState) -> usize {
    let mut last = state.last_tip_index.lock().unwrap();
    let count = REMINDER_PROMPT_COUNT.max(1);
    let mut rng = rand::thread_rng();
    let mut idx = rng.gen_range(0..count);
    if let Some(prev) = *last {
        if count > 1 && idx == prev {
            idx = (idx + 1 + rng.gen_range(0..(count - 1))) % count;
        }
    }
    *last = Some(idx);
    idx
}

#[tauri::command]
fn next_reminder_tip_text(state: State<'_, AppState>) -> String {
    let idx = next_tip_index_from_state(&state);
    REMINDER_TIPS_EN[idx % REMINDER_TIPS_EN.len()].to_string()
}

fn normalize_theme(theme: &str) -> String {
    if theme == "day" {
        "day".to_string()
    } else {
        "night".to_string()
    }
}

#[tauri::command]
fn set_theme(app: AppHandle, theme: String, state: State<'_, AppState>) -> Result<(), String> {
    let normalized = normalize_theme(&theme);
    {
        let mut t = state.theme.lock().unwrap();
        *t = normalized.clone();
    }

    let minutes = (*state.interval.lock().unwrap()) / 60;
    let ui_language = state.language.lock().unwrap().clone();
    let reminder_language = state.reminder_language.lock().unwrap().clone();
    save_config(&app, minutes, &ui_language, &reminder_language, &normalized);
    let _ = app.emit("theme-changed", normalized);
    Ok(())
}

#[tauri::command]
fn get_theme(state: State<'_, AppState>) -> String {
    state.theme.lock().unwrap().clone()
}

#[tauri::command]
fn get_active_reminder(state: State<'_, AppState>) -> ActiveReminderPayload {
    ActiveReminderPayload {
        id: *state.active_reminder_id.lock().unwrap(),
        text: state.active_reminder_tip.lock().unwrap().clone(),
        theme: state.theme.lock().unwrap().clone(),
        visible: *state.reminder_visible.lock().unwrap(),
    }
}

#[tauri::command]
fn get_system_language() -> String {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Globalization::GetUserDefaultUILanguage;
        // PRIMARYLANGID(langid)
        let lang_id = unsafe { GetUserDefaultUILanguage() } as u16;
        let primary = lang_id & 0x03ff;
        return if primary == 0x04 {
            "zh-CN".to_string()
        } else {
            "en".to_string()
        };
    }

    #[cfg(not(target_os = "windows"))]
    {
        let locale = sys_locale::get_locale()
            .unwrap_or_else(|| "en-US".to_string())
            .to_lowercase();
        if locale.starts_with("zh") {
            "zh-CN".to_string()
        } else {
            "en".to_string()
        }
    }
}

#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if !path.exists() {
        return Err("path not found".to_string());
    }

    #[cfg(target_os = "windows")]
    {
        let arg = format!("/select,{}", path.display());
        ProcessCommand::new("explorer")
            .arg(arg)
            .spawn()
            .map_err(|e| format!("open explorer failed: {}", e))?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        ProcessCommand::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("reveal failed: {}", e))?;
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.clone());
        ProcessCommand::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| format!("open folder failed: {}", e))?;
        return Ok(());
    }
}

#[tauri::command]
fn log_standup(app: AppHandle, state: State<'_, AppState>) -> u32 {
    let mut elapsed = state.elapsed.lock().unwrap();
    *elapsed = 0;
    *state.reminder_visible.lock().unwrap() = false;

    let now = now_ts();
    {
        let mut standups = state.standup_events.lock().unwrap();
        standups.push(now);
    }

    save_analytics(&app, &state);
    let analytics = build_analytics(&state);

    let _ = app.emit("standup-logged", ());
    let _ = app.emit("analytics-updated", ());
    analytics.standup_sessions
}

#[tauri::command]
fn acknowledge_reminder(
    app: AppHandle,
    stood_up: bool,
    reminder_id: Option<u64>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let active_id = *state.active_reminder_id.lock().unwrap();
    if let Some(id) = reminder_id {
        if id != active_id {
            return Ok(());
        }
    }
    if !*state.reminder_visible.lock().unwrap() {
        return Ok(());
    }

    // Ignore very early clicks to prevent accidental auto-dismiss right after show.
    if let Some(shown_at) = *state.active_reminder_shown_at.lock().unwrap() {
        if shown_at.elapsed() < Duration::from_millis(700) {
            return Ok(());
        }
    }

    let now = now_ts();
    let start_ts = *state.active_reminder_start_ts.lock().unwrap();
    let mut logged_sedentary = state.active_reminder_logged_sedentary.lock().unwrap();
    let mut wrote_analytics = false;

    if let Some(start) = start_ts {
        let lag = (now - start).max(0) as u64;
        if !*logged_sedentary && lag >= 60 {
            let interval_secs = *state.active_reminder_interval_secs.lock().unwrap();
            {
                let mut reminders = state.reminder_events.lock().unwrap();
                reminders.push(ReminderRecord {
                    ts: start,
                    duration_secs: interval_secs,
                });
            }
            *logged_sedentary = true;
            wrote_analytics = true;
        } else if !*logged_sedentary && stood_up {
            let mut standups = state.standup_events.lock().unwrap();
            standups.push(now);
            wrote_analytics = true;
        }
    } else if stood_up {
        let mut standups = state.standup_events.lock().unwrap();
        standups.push(now);
        wrote_analytics = true;
    }

    {
        let mut elapsed = state.elapsed.lock().unwrap();
        *elapsed = 0;
    }
    {
        let mut visible = state.reminder_visible.lock().unwrap();
        *visible = false;
    }
    {
        let mut active_start = state.active_reminder_start_ts.lock().unwrap();
        *active_start = None;
    }
    {
        let mut shown_at = state.active_reminder_shown_at.lock().unwrap();
        *shown_at = None;
    }

    if wrote_analytics {
        save_analytics(&app, &state);
        let _ = app.emit("analytics-updated", ());
        if stood_up {
            let _ = app.emit("standup-logged", ());
        }
    }

    if let Some(w) = app.get_webview_window("reminder") {
        let _ = w.hide();
    }
    Ok(())
}

#[tauri::command]
fn get_standup_count(state: State<'_, AppState>) -> u32 {
    build_analytics(&state).standup_sessions
}

#[tauri::command]
fn get_analytics(state: State<'_, AppState>, period: Option<String>) -> AnalyticsData {
    build_analytics_for_period(&state, period.as_deref().unwrap_or("daily"))
}

#[tauri::command]
fn export_analytics_csv(
    app: AppHandle,
    state: State<'_, AppState>,
    period: Option<String>,
) -> Result<String, String> {
    let period_key = normalize_period(period.as_deref().unwrap_or("daily"));
    let analytics = build_analytics_for_period(&state, period_key);
    if analytics.record_count < MIN_EXPORT_RECORDS {
        return Err(format!("NOT_ENOUGH_DATA:{}", MIN_EXPORT_RECORDS));
    }

    let mut rows = vec!["hour,sedentary_sessions,standup_sessions".to_string()];
    for hour in 0..HOURS {
        rows.push(format!(
            "{:02}:00,{},{}",
            hour,
            analytics.hourly_sedentary[hour],
            analytics.hourly_standup[hour]
        ));
    }
    rows.push(format!(
        "totals,{},{}",
        analytics.sedentary_sessions, analytics.standup_sessions
    ));
    rows.push(format!(
        "total_sitting_minutes,{},",
        (analytics.total_sitting_secs / 60)
    ));

    let now = Local::now();
    let file_name = format!(
        "standby_{}_analytics_{}.csv",
        period_key,
        now.format("%Y%m%d_%H%M%S")
    );
    let export_path = export_dir(&app)
        .ok_or_else(|| "cannot resolve export directory".to_string())?
        .join(file_name);

    if let Some(parent) = export_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&export_path, rows.join("\n")).map_err(|e| format!("write failed: {}", e))?;
    Ok(export_path.display().to_string())
}

#[tauri::command]
fn export_analytics_png(app: AppHandle, data_url: String) -> Result<String, String> {
    let payload = data_url
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| "invalid png payload".to_string())?;

    let png_bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("decode failed: {}", e))?;

    let now = Local::now();
    let file_name = format!("standby_24h_heatmap_{}.png", now.format("%Y%m%d_%H%M%S"));
    let export_path = export_dir(&app)
        .ok_or_else(|| "cannot resolve export directory".to_string())?
        .join(file_name);

    if let Some(parent) = export_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    fs::write(&export_path, png_bytes).map_err(|e| format!("write failed: {}", e))?;
    Ok(export_path.display().to_string())
}

#[tauri::command]
fn reset_daily_records(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let start_ts = period_start_ts("daily", Local::now());
    {
        let mut reminders = state.reminder_events.lock().unwrap();
        reminders.retain(|e| e.ts < start_ts);
    }
    {
        let mut standups = state.standup_events.lock().unwrap();
        standups.retain(|ts| *ts < start_ts);
    }
    save_analytics(&app, &state);
    let _ = app.emit("analytics-updated", ());
    Ok(())
}

#[tauri::command]
fn window_minimize(app: AppHandle, label: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&label) {
        w.minimize()
            .map_err(|e| format!("minimize failed: {}", e))?;
        return Ok(());
    }
    Err("window not found".into())
}

#[tauri::command]
fn window_toggle_maximize(app: AppHandle, label: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&label) {
        match w.is_maximized() {
            Ok(true) => {
                w.unmaximize()
                    .map_err(|e| format!("unmaximize failed: {}", e))?;
                Ok(())
            }
            Ok(false) => {
                w.maximize().map_err(|e| format!("maximize failed: {}", e))?;
                Ok(())
            }
            Err(e) => Err(format!("state query failed: {}", e)),
        }
    } else {
        Err("window not found".into())
    }
}

#[tauri::command]
fn window_close(app: AppHandle, label: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&label) {
        if label == "settings" {
            w.hide().map_err(|e| format!("hide failed: {}", e))?;
        } else {
            w.close().map_err(|e| format!("close failed: {}", e))?;
        }
        return Ok(());
    }
    Err("window not found".into())
}

#[tauri::command]
fn window_hide(app: AppHandle, label: String) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(&label) {
        w.hide().map_err(|e| format!("hide failed: {}", e))?;
        return Ok(());
    }
    Err("window not found".into())
}

fn show_or_create_settings_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    let created = WebviewWindowBuilder::new(
        app,
        "settings",
        WebviewUrl::App("settings.html".into()),
    )
    .title("Upstand Dashboard")
    .inner_size(980.0, 700.0)
    .decorations(false)
    .transparent(false)
    .center()
    .build();

    if let Ok(win) = created {
        let _ = win.show();
        let _ = win.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_or_create_settings_window(app);
        }))
        .manage(AppState {
            interval: Mutex::new(DEFAULT_INTERVAL_MINUTES * 60),
            elapsed: Mutex::new(0),
            last_interval_change: Mutex::new(Instant::now()),
            reminder_events: Mutex::new(Vec::new()),
            standup_events: Mutex::new(Vec::new()),
            reminder_visible: Mutex::new(false),
            language: Mutex::new("en".to_string()),
            reminder_language: Mutex::new("en".to_string()),
            theme: Mutex::new("night".to_string()),
            last_tip_index: Mutex::new(None),
            active_reminder_id: Mutex::new(0),
            active_reminder_start_ts: Mutex::new(None),
            active_reminder_shown_at: Mutex::new(None),
            active_reminder_interval_secs: Mutex::new(DEFAULT_INTERVAL_MINUTES * 60),
            active_reminder_logged_sedentary: Mutex::new(false),
            active_reminder_tip: Mutex::new("Time to stand up and stretch.".to_string()),
        })
        .setup(|app| {
            let app_handle = app.handle().clone();

            let state = app.state::<AppState>();
            load_config(&app_handle, &state);
            load_analytics(&app_handle, &state);
            let startup_lang = state.language.lock().unwrap().clone();

            let tray_menu = make_tray_menu(&app_handle, &startup_lang)?;

            let tray_icon = Image::from_path("icons/icon-16.png")
                .or_else(|_| Image::from_path("icons/icon-32.png"))
                .ok()
                .or_else(|| app.default_window_icon().cloned())
                .ok_or("missing tray icon")?;

            let tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(tray_icon)
                .menu(&tray_menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open_settings" => {
                        show_or_create_settings_window(app);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            std::mem::forget(tray);

            let handle_for_splash = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs_f32(2.6)).await;
                if let Some(main_win) = handle_for_splash.get_webview_window("main") {
                    let _ = main_win.close();
                }
                show_or_create_settings_window(&handle_for_splash);
            });

            let reminder_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;

                    let state = reminder_handle.state::<AppState>();
                    if *state.reminder_visible.lock().unwrap() {
                        if let Some(rw) = reminder_handle.get_webview_window("reminder") {
                            if let Ok(false) = rw.is_visible() {
                                let _ = rw.show();
                                let _ = rw.set_focus();
                                let reminder_id = *state.active_reminder_id.lock().unwrap();
                                let _ = rw.emit("refresh_tip", reminder_id);
                            }
                        } else {
                            *state.reminder_visible.lock().unwrap() = false;
                            *state.active_reminder_start_ts.lock().unwrap() = None;
                            *state.active_reminder_shown_at.lock().unwrap() = None;
                            continue;
                        }

                        let maybe_new_sedentary = {
                            let start_opt = *state.active_reminder_start_ts.lock().unwrap();
                            let mut logged = state.active_reminder_logged_sedentary.lock().unwrap();
                            if let Some(start) = start_opt {
                                let lag = (now_ts() - start).max(0) as u64;
                                if !*logged && lag >= 60 {
                                    *logged = true;
                                    Some((start, lag))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        };
                        if let Some((start, _lag)) = maybe_new_sedentary {
                            let interval_secs = *state.active_reminder_interval_secs.lock().unwrap();
                            {
                                let mut reminders = state.reminder_events.lock().unwrap();
                                reminders.push(ReminderRecord {
                                    ts: start,
                                    duration_secs: interval_secs,
                                });
                            }
                            save_analytics(&reminder_handle, &state);
                            let _ = reminder_handle.emit("analytics-updated", ());
                        }
                        continue;
                    }
                    let mut elapsed = state.elapsed.lock().unwrap();
                    *elapsed += 5;

                    let current_limit = *state.interval.lock().unwrap();

                    if *elapsed >= current_limit {
                        if let Some(rw) = reminder_handle.get_webview_window("reminder") {
                            let reminder_id = {
                                let mut id = state.active_reminder_id.lock().unwrap();
                                *id += 1;
                                *id
                            };
                            let tip_index = next_tip_index_from_state(&state);
                            let tip = REMINDER_TIPS_EN[tip_index].to_string();
                            {
                                let mut tip_slot = state.active_reminder_tip.lock().unwrap();
                                *tip_slot = tip;
                            }
                            {
                                let mut start = state.active_reminder_start_ts.lock().unwrap();
                                *start = Some(now_ts());
                            }
                            {
                                let mut shown_at = state.active_reminder_shown_at.lock().unwrap();
                                *shown_at = Some(Instant::now());
                            }
                            {
                                let mut interval_secs = state.active_reminder_interval_secs.lock().unwrap();
                                *interval_secs = current_limit;
                            }
                            {
                                let mut logged = state.active_reminder_logged_sedentary.lock().unwrap();
                                *logged = false;
                            }

                            let _ = rw.set_size(tauri::Size::Physical(tauri::PhysicalSize::new(
                                REMINDER_WIDTH as u32,
                                REMINDER_HEIGHT as u32,
                            )));

                            // Prefer primary monitor for taskbar/tray anchoring.
                            let monitor = reminder_handle
                                .primary_monitor()
                                .ok()
                                .flatten()
                                .or_else(|| rw.current_monitor().ok().flatten());

                            if let Some(monitor) = monitor {
                                let margin = 28i32;
                                let area = monitor.work_area();
                                let area_pos = area.position;
                                let area_size = area.size;
                                let size = rw
                                    .outer_size()
                                    .ok()
                                    .map(|s| (s.width as i32, s.height as i32))
                                    .unwrap_or((REMINDER_WIDTH, REMINDER_HEIGHT));

                                let x = area_pos.x + (area_size.width as i32) - size.0 - margin;
                                let y = area_pos.y + (area_size.height as i32) - size.1 - margin;

                                let _ = rw.set_position(PhysicalPosition::new(x, y));
                            }

                            *state.reminder_visible.lock().unwrap() = true;
                            let _ = rw.show();
                            let _ = rw.set_focus();
                            let _ = rw.emit("refresh_tip", reminder_id);
                            let _ = rw.eval("window.__standbyReminderSync && window.__standbyReminderSync();");
                        }
                        let _ = reminder_handle.emit("reminder-fired", ());

                        *elapsed = 0;
                    }
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_reminder_interval,
            get_reminder_interval,
            log_standup,
            acknowledge_reminder,
            get_standup_count,
            get_analytics,
            export_analytics_csv,
            export_analytics_png,
            reset_daily_records,
            set_language,
            get_language,
            set_reminder_language,
            get_reminder_language,
            next_reminder_tip_index,
            next_reminder_tip_text,
            get_active_reminder,
            get_system_language,
            set_theme,
            get_theme,
            reveal_in_explorer,
            window_minimize,
            window_toggle_maximize,
            window_close,
            window_hide
        ])
        .run(tauri::generate_context!())
        .expect("error while running standby");
}
