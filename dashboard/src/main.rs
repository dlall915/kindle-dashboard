use std::io::Write;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use slint::{ModelRc, SharedString, VecModel};
use slint_backend_kindle::WakeSchedule;

slint::include_modules!();

static DEFAULT_FONT: &[u8] = include_bytes!("../ui/LiberationSans-Regular.ttf");

const HA_URL_FILE: &str = "/mnt/us/extensions/weather_dashboard/ha_url.txt";
const HA_URL_DEFAULT: &str = "http://homeassistant.local:8123";
const TOKEN_FILE: &str = "/mnt/us/extensions/weather_dashboard/token.txt";
const LOG_FILE: &str = "/mnt/us/extensions/weather_dashboard/dashboard.log";
const CALENDAR_ENTITY: &str = "calendar.household";

/// (case-insensitive substring to match in an event's summary, icon key
/// under ui/icons/, display message). A match becomes a bottom-section
/// icon+text alert instead of a plain line in the regular "today" list.
/// Adding a future alert is one new row here plus one new icon file - see
/// README.md.
const ALERT_KEYWORDS: &[(&str, &str, &str)] = &[
    ("trash", "trash", "Take out the trash!"),
    ("recycling", "recycling", "Take out the recycling!"),
    ("leaves", "leaves", "Take out the leaves!"),
];

fn log_line(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_FILE)
    {
        let _ = writeln!(f, "{line}");
    }
}

fn now(fmt: &str) -> String {
    Command::new("date")
        .arg(format!("+{fmt}"))
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn read_token() -> Option<String> {
    std::fs::read_to_string(TOKEN_FILE)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Reads the target Home Assistant URL from `ha_url.txt` next to the
/// binary, falling back to a generic placeholder if that file doesn't
/// exist - keeps a real deployment's actual LAN address out of source
/// control while still letting the app run out of the box.
fn read_ha_url() -> String {
    std::fs::read_to_string(HA_URL_FILE)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| HA_URL_DEFAULT.to_string())
}

/// Fetch a single entity's state JSON from Home Assistant. Returns None on
/// any failure (network down, bad token, entity missing) - callers fall
/// back to a sensible default rather than propagating the error, since a
/// single failed refresh isn't worth crashing the whole dashboard over.
fn fetch_state(ha_url: &str, token: &str, entity_id: &str) -> Option<Value> {
    let url = format!("{ha_url}/api/states/{entity_id}");
    let response = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
        .ok()?;
    response.into_json().ok()
}

/// Map a Home Assistant weather.condition value to one of our icon keys.
/// Several conditions intentionally share an icon (see icons/ - only 11
/// distinct icon files exist for ~15 possible condition values).
fn icon_key_for(condition: &str) -> &'static str {
    match condition {
        "sunny" => "sunny",
        "clear-night" => "clear-night",
        "partlycloudy" => "partlycloudy",
        "cloudy" => "cloudy",
        "rainy" => "rainy",
        "pouring" => "pouring",
        "lightning" | "lightning-rainy" => "lightning",
        "snowy" | "snowy-rainy" => "snowy",
        "fog" | "exceptional" => "fog",
        "windy" | "windy-variant" => "windy",
        "hail" => "hail",
        _ => "default",
    }
}

/// Human-readable label for a raw HA condition string ("partlycloudy" ->
/// "Partly Cloudy"). Kept separate from icon_key_for: icon selection and
/// display wording don't need to share the same groupings.
fn display_name_for(condition: &str) -> String {
    match condition {
        "sunny" => "Sunny".to_string(),
        "clear-night" => "Clear".to_string(),
        "partlycloudy" => "Partly Cloudy".to_string(),
        "cloudy" => "Cloudy".to_string(),
        "rainy" => "Rainy".to_string(),
        "pouring" => "Pouring".to_string(),
        "lightning" => "Thunderstorms".to_string(),
        "lightning-rainy" => "Thunderstorms & Rain".to_string(),
        "snowy" => "Snowy".to_string(),
        "snowy-rainy" => "Snow & Rain".to_string(),
        "fog" => "Foggy".to_string(),
        "exceptional" => "Severe Weather".to_string(),
        "windy" => "Windy".to_string(),
        "windy-variant" => "Windy".to_string(),
        "hail" => "Hail".to_string(),
        other => {
            // Fallback for anything unmapped: turn "some-condition" into
            // "Some Condition" rather than showing the raw HA slug.
            other
                .split(['-', '_'])
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
    }
}

/// Stop the stock Kindle screensaver/lock image from taking over the
/// framebuffer. This app draws straight to the framebuffer and has no
/// window-manager-level way to keep the OS's own screensaver from
/// drawing over it - so without this, the device shows the stock lock
/// screen instead of the dashboard whenever powerd's idle screensaver
/// fires (observed in practice after a manual `preventScreenSaver 0`
/// during testing). Re-asserted on every refresh rather than once at
/// startup in case anything else on the device resets the property.
fn disable_stock_screensaver() {
    let _ = Command::new("lipc-set-prop")
        .args(["com.lab126.powerd", "preventScreenSaver", "1"])
        .output();
}

/// This Kindle Paperwhite 2 has `canTurnFrontlightOff = false` in KOReader's
/// own device profile - on this hardware, setting `flIntensity` to 0 over
/// lipc does *not* actually turn the LED off, it just dims it (confirmed
/// on-device: lipc reported 0 while the raw backlight sysfs file still read
/// 1 out of a 4095 max, a small but visible glow - matches KOReader's own
/// code comment about "the fl restore on resume less jarring on devices
/// where lipc 0 != off"). powerd appears to restore some frontlight level
/// on each resume from suspend, which is the actual "the screen lights up
/// on wake" behavior reported by the user, since this app never otherwise
/// touches the frontlight at all. Fixed with the same two-part approach
/// KOReader uses for this exact quirk: the lipc call (harmless, kept for
/// symmetry) plus a direct write to the raw backlight sysfs file, which is
/// what actually kills the LED on this device. Re-asserted on every
/// refresh, not just startup, since the resume behavior is exactly what
/// needs correcting each time.
const FRONTLIGHT_SYSFS: &str = "/sys/class/backlight/max77696-bl/brightness";
fn disable_frontlight() {
    let _ = Command::new("lipc-set-prop")
        .args(["com.lab126.powerd", "flIntensity", "0"])
        .output();
    let _ = std::fs::write(FRONTLIGHT_SYSFS, "0");
}

/// Stop the stock Kindle status bar (WiFi icon + system clock, drawn by
/// the "pillow" compositor overlay) from bleeding through in the top-right
/// corner over our own framebuffer content - confirmed happening in
/// practice via an on-device photo. Same two commands KOReader's own
/// launch script (`koreader.sh`) uses before it starts drawing, run once
/// at startup rather than every refresh like `disable_stock_screensaver` -
/// this is a compositor mode toggle, not an idle timer, so it's not
/// expected to reset itself the way the screensaver property did.
fn disable_pillow_overlay() {
    let _ = Command::new("lipc-set-prop")
        .args(["com.lab126.pillow", "disableEnablePillow", "disable"])
        .output();
    let _ = Command::new("lipc-set-prop")
        .args([
            "com.lab126.pillow",
            "interrogatePillow",
            r#"{"pillowId": "default_status_bar", "function": "nativeBridge.hideMe();"}"#,
        ])
        .output();
}

/// Add one calendar day to a "YYYY-MM-DD" string. Pure arithmetic - no
/// chrono/time dependency needed for this one calculation, and this
/// Kindle's busybox `date` has no GNU `-d` relative-date support to lean on
/// instead (confirmed earlier cross-compiling/testing on this device).
fn next_day(date: &str) -> String {
    let parts: Vec<i32> = date.split('-').filter_map(|s| s.parse().ok()).collect();
    let (mut y, mut m, mut d) = match parts.as_slice() {
        [y, m, d] => (*y, *m, *d),
        _ => return date.to_string(),
    };
    let is_leap = |y: i32| (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days_in_month = |y: i32, m: i32| match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 30,
    };
    d += 1;
    if d > days_in_month(y, m) {
        d = 1;
        m += 1;
        if m > 12 {
            m = 1;
            y += 1;
        }
    }
    format!("{y:04}-{m:02}-{d:02}")
}

/// Local UTC offset in "+HH:MM"/"-HH:MM" form, as HA's calendar API expects
/// - `date +%z` gives "+HHMM"/"-HHMM" instead, so reformat it.
fn utc_offset() -> String {
    let raw = now("%z");
    if raw.len() == 5 {
        format!("{}:{}", &raw[..3], &raw[3..])
    } else {
        "+00:00".to_string()
    }
}

/// Fetch today's events (the Kindle's own local day, not Home Assistant's
/// server day - they could disagree) from the household calendar. Returns
/// the raw event list; `refresh()` splits it into plain "today" lines vs.
/// icon alerts via `ALERT_KEYWORDS`.
fn fetch_todays_events(ha_url: &str, token: &str) -> Vec<Value> {
    let today = now("%F");
    let offset = utc_offset();
    let start = format!("{today}T00:00:00{offset}");
    let end = format!("{}T00:00:00{offset}", next_day(&today));
    let url = format!("{ha_url}/api/calendars/{CALENDAR_ENTITY}?start={start}&end={end}");
    let response = match ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    response.into_json::<Vec<Value>>().unwrap_or_default()
}

/// Format one calendar event for the plain "today" list: "3:00 PM  Dentist"
/// for a timed event, just the summary for an all-day one.
fn format_event_text(event: &Value) -> Option<String> {
    let summary = event["summary"].as_str()?.to_string();
    if let Some(datetime) = event["start"]["dateTime"].as_str() {
        let time_part = datetime.get(11..16)?;
        let (h, m) = time_part.split_once(':')?;
        let mut hour: i32 = h.parse().ok()?;
        let am_pm = if hour >= 12 { "PM" } else { "AM" };
        if hour == 0 {
            hour = 12;
        } else if hour > 12 {
            hour -= 12;
        }
        Some(format!("{hour}:{m} {am_pm}  {summary}"))
    } else {
        Some(summary)
    }
}

/// Check an event's summary against `ALERT_KEYWORDS` (case-insensitive
/// substring match). Returns (icon key, display message) on a match.
fn match_alert(summary: &str) -> Option<(&'static str, &'static str)> {
    let lower = summary.to_lowercase();
    ALERT_KEYWORDS
        .iter()
        .find(|(keyword, _, _)| lower.contains(keyword))
        .map(|(_, icon, message)| (*icon, *message))
}

fn battery_percent() -> Option<String> {
    let output = Command::new("lipc-get-prop")
        .args(["com.lab126.powerd", "battLevel"])
        .output()
        .ok()?;
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(format!("{value}%"))
    }
}

/// One full refresh: fetch weather + washer status, update every UI
/// property. Called once at startup and again on every scheduled wake.
///
/// `on_wake` fires immediately on resume, not after any grace period -
/// `stay_awake` only governs how long the app must be idle BEFORE it's
/// allowed to suspend, not a delay after waking. WiFi needs a few seconds
/// to reassociate post-resume (confirmed the hard way in the shell-script
/// version, where skipping this made every fetch fail), so wait before any
/// network call. Only applied on wake calls, not the initial startup call.
fn refresh_after_wake(app: &AppWindow) {
    std::thread::sleep(Duration::from_secs(4));
    refresh(app);
}

fn refresh(app: &AppWindow) {
    disable_stock_screensaver();
    disable_frontlight();
    app.set_time_text(now("%I:%M %p").into());
    app.set_date_text(now("%A, %B %d").into());
    if let Some(batt) = battery_percent() {
        app.set_battery_text(batt.into());
    }

    let Some(token) = read_token() else {
        log_line(&format!("{} refresh: missing token file", now("%F %T")));
        app.set_weather_text("(no token)".into());
        return;
    };
    let ha_url = read_ha_url();

    let washer_done = fetch_state(&ha_url, &token, "input_boolean.basement_washing_machine_done")
        .and_then(|v| v["state"].as_str().map(|s| s == "on"))
        .unwrap_or(false);
    app.set_washer_done(washer_done);

    if washer_done {
        log_line(&format!("{} refresh: washer done, alert shown", now("%F %T")));
        return;
    }

    match fetch_state(&ha_url, &token, "weather.home") {
        Some(v) => {
            let condition = v["state"].as_str().unwrap_or("(offline)").to_string();
            let temp = v["attributes"]["temperature"].as_f64();
            app.set_weather_icon(icon_key_for(&condition).into());
            let label = display_name_for(&condition);
            let text = match temp {
                Some(t) => format!("{t}\u{00B0}F, {label}"),
                None => label,
            };
            app.set_weather_text(text.into());
            log_line(&format!("{} refresh: ok", now("%F %T")));
        }
        None => {
            app.set_weather_icon("default".into());
            app.set_weather_text("(offline)".into());
            log_line(&format!("{} refresh: weather fetch failed", now("%F %T")));
        }
    }

    let events = fetch_todays_events(&ha_url, &token);
    let mut today_items: Vec<SharedString> = Vec::new();
    let mut today_alerts: Vec<AlertItem> = Vec::new();
    for event in &events {
        let summary = event["summary"].as_str().unwrap_or("");
        if let Some((icon, message)) = match_alert(summary) {
            today_alerts.push(AlertItem {
                icon: icon.into(),
                text: message.into(),
            });
        } else if let Some(text) = format_event_text(event) {
            today_items.push(text.into());
        }
    }
    log_line(&format!(
        "{} refresh: {} today item(s), {} alert(s)",
        now("%F %T"),
        today_items.len(),
        today_alerts.len()
    ));
    app.set_today_items(ModelRc::new(VecModel::from(today_items)));
    app.set_today_alerts(ModelRc::new(VecModel::from(today_alerts)));
}

fn main() {
    disable_pillow_overlay();
    let backend =
        slint_backend_kindle::install(DEFAULT_FONT).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");

    log_line(&format!("{} start", now("%F %T")));
    refresh(&app);

    // Temporarily lowered from 600s to 300s during calendar/alerts testing,
    // to both observe battery impact at this cadence and iterate faster.
    let backend = backend.set_wake_schedule(WakeSchedule {
        wake_interval: Duration::from_secs(300),
        stay_awake: Duration::from_secs(8),
    });

    let app_weak = app.as_weak();
    backend.on_wake(move || {
        if let Some(app) = app_weak.upgrade() {
            refresh_after_wake(&app);
        }
    });

    // Ticks the clock and, same as the toolchain-spike found, keeps the
    // event loop cycling so the suspend check actually re-runs (it blocks
    // forever in poll(-1) otherwise when touch is unavailable).
    let app_weak_tick = app.as_weak();
    let tick_timer = slint::Timer::default();
    tick_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(1),
        move || {
            if let Some(app) = app_weak_tick.upgrade() {
                app.set_time_text(now("%I:%M %p").into());
            }
        },
    );

    app.run().expect("event loop error");
}
