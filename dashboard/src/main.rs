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

// ~1,150 log lines/day with no rotation grows unbounded (audit finding
// #23). 512KB holds many days of normal refresh logging at this app's
// line lengths - plenty to debug a recent problem, without growing
// forever on a device that is not expected to ever get rebooted to
// clear it.
const LOG_MAX_BYTES: u64 = 512 * 1024;

fn log_line(line: &str) {
    let over_limit = std::fs::metadata(LOG_FILE)
        .map(|m| m.len() > LOG_MAX_BYTES)
        .unwrap_or(false);
    let opened = if over_limit {
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(LOG_FILE)
    } else {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(LOG_FILE)
    };
    if let Ok(mut f) = opened {
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

/// Everything `refresh()` needs from `date`, in one subprocess spawn
/// instead of five separate ones (log timestamp, time_text, date_text,
/// today's ISO date for the calendar window, and the UTC offset for that
/// same window) - audit finding #18. The tick timer's own once/second
/// now() call is left as-is: consolidating it would risk the suspend-check
/// timing it also depends on (see its own comment), a much higher-risk
/// change than this one for a low-priority efficiency finding.
struct NowSnapshot {
    log_ts: String,
    time_text: String,
    date_text: String,
    iso_date: String,
    utc_offset: String,
}

fn now_snapshot() -> NowSnapshot {
    let raw = Command::new("date")
        .arg("+%F %T|%I:%M %p|%A, %B %d|%z")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let mut parts = raw.trim().split('|');
    let mut next = || parts.next().unwrap_or("?").to_string();
    let log_ts = next();
    let time_text = next();
    let date_text = next();
    // %F %T also gives us today's ISO date for free: everything before
    // the first space.
    let iso_date = log_ts.split(' ').next().unwrap_or("?").to_string();
    let raw_offset = next();
    let utc_offset = if raw_offset.len() == 5 {
        format!("{}:{}", &raw_offset[..3], &raw_offset[3..])
    } else {
        // A silent fallback here shifts the whole calendar day window - in
        // EDT that is 4 hours, enough to drop late-evening events or pull
        // in yesterday's. Log it: this should never actually fire, so a
        // log line costs nothing and a silent wrong answer is worse than
        // a loud wrong one (audit finding #14).
        log_line(&format!(
            "{log_ts} utc_offset: unexpected `date +%z` output {raw_offset:?}, falling back to +00:00"
        ));
        "+00:00".to_string()
    };
    NowSnapshot {
        log_ts,
        time_text,
        date_text,
        iso_date,
        utc_offset,
    }
}

fn read_token() -> Option<String> {
    std::fs::read_to_string(TOKEN_FILE)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// Shared connection pool for every HTTP request this app makes. A fresh
/// `ureq::get()` call built its own Agent each time, so all three requests
/// per refresh opened a new TCP connection instead of reusing one (audit
/// finding #20). A short, separate connect timeout also lets a truly dead
/// connection fail faster than the overall per-request timeout would.
fn http_agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .build()
    })
}

/// Fetch a single entity's state JSON from Home Assistant. Returns None on
/// any failure (network down, bad token, entity missing) - callers fall
/// back to a sensible default rather than propagating the error, since a
/// single failed refresh isn't worth crashing the whole dashboard over.
fn fetch_state(ha_url: &str, token: &str, entity_id: &str) -> Option<Value> {
    let url = format!("{ha_url}/api/states/{entity_id}");
    let response = match http_agent().get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            // Distinguishes an HTTP status (a rotated/expired token, a
            // renamed entity) from a transport error (WiFi down) - without
            // this, both looked identical in the log (audit finding #12).
            log_line(&format!(
                "{} fetch {entity_id}: request failed: {e}",
                now("%F %T")
            ));
            return None;
        }
    };
    match response.into_json() {
        Ok(v) => Some(v),
        Err(e) => {
            log_line(&format!(
                "{} fetch {entity_id}: bad JSON in response: {e}",
                now("%F %T")
            ));
            None
        }
    }
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
/// display wording don't need to share the same groupings. Returns a
/// borrowed &'static str for the 15 known conditions - only the fallback
/// case for an unmapped condition needs to build a real String (audit
/// finding #23).
fn display_name_for(condition: &str) -> std::borrow::Cow<'static, str> {
    match condition {
        "sunny" => "Sunny".into(),
        "clear-night" => "Clear".into(),
        "partlycloudy" => "Partly Cloudy".into(),
        "cloudy" => "Cloudy".into(),
        "rainy" => "Rainy".into(),
        "pouring" => "Pouring".into(),
        "lightning" => "Thunderstorms".into(),
        "lightning-rainy" => "Thunderstorms & Rain".into(),
        "snowy" => "Snowy".into(),
        "snowy-rainy" => "Snow & Rain".into(),
        "fog" => "Foggy".into(),
        "exceptional" => "Severe Weather".into(),
        "windy" => "Windy".into(),
        "windy-variant" => "Windy".into(),
        "hail" => "Hail".into(),
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
                .into()
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
/// touches the frontlight at all. Fixed with a direct write to the raw
/// backlight sysfs file, which is what actually kills the LED on this
/// device - the lipc call KOReader also makes for this same quirk is a
/// confirmed no-op here (audit finding #19), so this only does the part
/// that works. Re-asserted on every refresh, not just startup, since the
/// resume behavior is exactly what needs correcting each time.
const FRONTLIGHT_SYSFS: &str = "/sys/class/backlight/max77696-bl/brightness";
fn disable_frontlight() {
    let _ = std::fs::write(FRONTLIGHT_SYSFS, "0");
}

/// Stop the stock Kindle status bar (WiFi icon + system clock, drawn by
/// the "pillow" compositor overlay) from bleeding through in the top-right
/// corner over our own framebuffer content - confirmed happening in
/// practice via an on-device photo. Same two commands KOReader's own
/// launch script (`koreader.sh`) uses before it starts drawing. Originally
/// only called once at startup on the assumption that a compositor mode
/// toggle wouldn't reset itself the way the screensaver property did -
/// wrong, a second on-device photo after a wake cycle showed the overlay
/// back, same pattern as `disable_stock_screensaver`/`disable_frontlight`.
/// Re-asserted on every refresh now too.
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

/// Stop KOReader's own X11-based idle screensaver ("blanket" module
/// "screensaver", loaded by default) from drawing a small live clock into
/// this shared framebuffer - confirmed the actual source of a corner-clock
/// ghost via on-device testing (disabling "pillow" above did not fix it;
/// unloading this module did, verified clean across many wake cycles).
/// `blanket` manages several independent modules over lipc's load/unload
/// interface (splash, screensaver, langpicker, blankwindow); this touches
/// only "screensaver", leaving the others (for example the language
/// picker) untouched. This is a runtime-only unload with no persisted
/// config, so it must be re-applied on every startup - blanket reloads its
/// default module set on its own restart, not just once at boot.
fn disable_screensaver_module() {
    let _ = Command::new("lipc-set-prop")
        .args(["com.lab126.blanket", "unload", "-s", "screensaver"])
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

/// Fetch today's events (the Kindle's own local day, not Home Assistant's
/// server day - they could disagree) from the household calendar. Returns
/// the raw event list; `refresh()` splits it into plain "today" lines vs.
/// icon alerts via `ALERT_KEYWORDS`. `today` and `offset` come from the
/// caller's own `NowSnapshot` rather than being fetched again here (audit
/// finding #18).
fn fetch_todays_events(ha_url: &str, token: &str, today: &str, offset: &str) -> Vec<Value> {
    let start = format!("{today}T00:00:00{offset}");
    let end = format!("{}T00:00:00{offset}", next_day(today));
    let url = format!("{ha_url}/api/calendars/{CALENDAR_ENTITY}?start={start}&end={end}");
    let response = match http_agent().get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(10))
        .call()
    {
        Ok(r) => r,
        Err(e) => {
            // Both failure paths used to return an empty Vec with no log
            // line at all - identical to a genuinely empty calendar day
            // (audit finding #12). A token rotation would otherwise look
            // the same as "nothing on the calendar today" forever.
            log_line(&format!(
                "{} fetch {CALENDAR_ENTITY}: request failed: {e}",
                now("%F %T")
            ));
            return Vec::new();
        }
    };
    match response.into_json::<Vec<Value>>() {
        Ok(v) => v,
        Err(e) => {
            log_line(&format!(
                "{} fetch {CALENDAR_ENTITY}: bad JSON in response: {e}",
                now("%F %T")
            ));
            Vec::new()
        }
    }
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

/// Re-apply the stock-behavior overrides needed on this device: the stock
/// screensaver, the frontlight, and the pillow status-bar overlay all reset
/// on every suspend/resume cycle and need reasserting each wake. The
/// blanket screensaver module (below) does not - one on-device test with a
/// single unload call stayed clean across 10+ wake cycles - but it is
/// cheap to repeat and this guards against blanket reloading its default
/// module set for some other reason this app does not control. Called as
/// early as possible on every wake - before the WiFi
/// reassociation sleep in `refresh_after_wake`, not after it - since these
/// are local `Command`/sysfs calls with no network dependency, and the
/// whole point is correcting stock behavior before the user can see it.
/// Also called once at startup for the same reason.
fn reassert_device_state() {
    disable_stock_screensaver();
    disable_frontlight();
    disable_pillow_overlay();
    disable_screensaver_module();
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
    reassert_device_state();
    std::thread::sleep(Duration::from_secs(4));
    refresh(app);
}

fn refresh(app: &AppWindow) {
    let snap = now_snapshot();
    app.set_time_text(snap.time_text.into());
    app.set_date_text(snap.date_text.into());
    if let Some(batt) = battery_percent() {
        app.set_battery_text(batt.into());
    }

    let Some(token) = read_token() else {
        log_line(&format!("{} refresh: missing token file", snap.log_ts));
        app.set_weather_text("(no token)".into());
        // Clear stale content instead of leaving it on screen next to
        // "(no token)" - a deleted or truncated token file can happen
        // while a trash alert or washer-done alert is up, and neither
        // should stay showing once this app can no longer fetch anything
        // to confirm it's still accurate (audit finding #15).
        app.set_today_items(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        app.set_today_alerts(ModelRc::new(VecModel::from(Vec::<AlertItem>::new())));
        app.set_washer_done(false);
        return;
    };
    let ha_url = read_ha_url();

    // Keep the previous washer_done value on a fetch failure, instead of
    // defaulting to false. A dropped connection must not look the same as
    // "the washer is off" - that would silently clear a real alert still
    // on screen the moment WiFi hiccups, even though HA still holds the
    // helper on.
    let washer_done = match fetch_state(&ha_url, &token, "input_boolean.basement_washing_machine_done")
    {
        Some(v) => v["state"]
            .as_str()
            .map(|s| s == "on")
            .unwrap_or(app.get_washer_done()),
        None => {
            log_line(&format!(
                "{} refresh: washer status fetch failed, keeping previous state",
                snap.log_ts
            ));
            app.get_washer_done()
        }
    };
    app.set_washer_done(washer_done);

    if washer_done {
        log_line(&format!("{} refresh: washer done, alert shown", snap.log_ts));
        return;
    }

    match fetch_state(&ha_url, &token, "weather.home") {
        Some(v) => {
            let condition = v["state"].as_str().unwrap_or("(offline)").to_string();
            let temp = v["attributes"]["temperature"].as_f64();
            app.set_weather_icon(icon_key_for(&condition).into());
            let label = display_name_for(&condition);
            let text = match temp {
                // {t:.0} avoids an occasional 71.60000000000001°F from raw
                // float formatting (audit finding #23).
                Some(t) => format!("{t:.0}\u{00B0}F, {label}"),
                None => label.into_owned(),
            };
            app.set_weather_text(text.into());
            log_line(&format!("{} refresh: ok", snap.log_ts));
        }
        None => {
            app.set_weather_icon("default".into());
            app.set_weather_text("(offline)".into());
            log_line(&format!("{} refresh: weather fetch failed", snap.log_ts));
        }
    }

    let events = fetch_todays_events(&ha_url, &token, &snap.iso_date, &snap.utc_offset);
    let mut today_items: Vec<SharedString> = Vec::new();
    let mut today_alerts: Vec<AlertItem> = Vec::new();
    for event in &events {
        let summary = event["summary"].as_str().unwrap_or("");
        // Alert keywords only match all-day events. Every real
        // trash/recycling/leaves reminder is all-day; restricting to that
        // avoids ordinary timed-event phrasing being mistaken for one
        // ("Flight leaves 6am", "Sitter leaves at 4") - a plain substring
        // match on "leaves" would otherwise both show a false alert *and*
        // silently remove the real event from today_items.
        let is_all_day = event["start"]["dateTime"].as_str().is_none();
        if let (true, Some((icon, message))) = (is_all_day, match_alert(summary)) {
            // One alert per keyword per day. A duplicate reminder (two
            // separate all-day events that both match "trash", for
            // example) does not need a second identical row - and two
            // rows with byte-identical text were observed, on-device, to
            // sometimes render as one instead of two. Deduping here
            // avoids depending on that render behavior at all.
            if !today_alerts.iter().any(|a| a.icon == icon) {
                today_alerts.push(AlertItem {
                    icon: icon.into(),
                    text: message.into(),
                });
            }
        } else if let Some(text) = format_event_text(event) {
            today_items.push(text.into());
        }
    }
    // Cap the plain item list so it cannot push the alerts section off the
    // bottom of the 758x1024 panel (audit finding #6). app.slint now
    // shrinks the item font size as the list grows, so this cap only
    // needs to be a generous safety limit, not the primary fix - most of
    // the "fit more on screen" work happens there. The limit still gets
    // tighter as more alerts are showing, since each alert icon+message
    // row costs much more vertical room than a plain item line.
    let item_cap: usize = match today_alerts.len() {
        0 => usize::MAX, // no bottom section at all in this case - see app.slint
        1 => 8,
        2 => 5,
        _ => 2,
    };
    if today_items.len() > item_cap {
        let hidden = today_items.len() - item_cap;
        today_items.truncate(item_cap);
        today_items.push(format!("+{hidden} more").into());
    }

    log_line(&format!(
        "{} refresh: {} today item(s), {} alert(s)",
        snap.log_ts,
        today_items.len(),
        today_alerts.len()
    ));
    app.set_today_items(ModelRc::new(VecModel::from(today_items)));
    app.set_today_alerts(ModelRc::new(VecModel::from(today_alerts)));
}

fn main() {
    let backend =
        slint_backend_kindle::install(DEFAULT_FONT).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");

    log_line(&format!("{} start", now("%F %T")));
    reassert_device_state();
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
