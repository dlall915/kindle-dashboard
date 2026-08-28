use std::io::Write;
use std::process::Command;
use std::time::Duration;

use serde_json::Value;
use slint_backend_kindle::WakeSchedule;

slint::include_modules!();

static DEFAULT_FONT: &[u8] = include_bytes!("../ui/LiberationSans-Regular.ttf");

const HA_URL_FILE: &str = "/mnt/us/extensions/weather_dashboard/ha_url.txt";
const HA_URL_DEFAULT: &str = "http://homeassistant.local:8123";
const TOKEN_FILE: &str = "/mnt/us/extensions/weather_dashboard/token.txt";
const LOG_FILE: &str = "/mnt/us/extensions/weather_dashboard/dashboard.log";

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
}

fn main() {
    let backend =
        slint_backend_kindle::install(DEFAULT_FONT).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");

    log_line(&format!("{} start", now("%F %T")));
    refresh(&app);

    // Matches the shell-script loop's cadence.
    let backend = backend.set_wake_schedule(WakeSchedule {
        wake_interval: Duration::from_secs(600),
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
