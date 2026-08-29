# Kindle Dashboard

This is a dashboard app for a Kindle Paperwhite 2 with custom firmware.
The app is native code, written in Rust and Slint. The Kindle shows the
dashboard as a low-power e-ink display: the time, the weather, today's
calendar items from Home Assistant, and a washing-machine-done alert.
The app reads this data from a local Home Assistant instance. The app
draws directly to the Kindle's framebuffer. It does not use a browser
or X11.

This app replaces an earlier version that used a shell script and
FBInk. That version still works. It is in `legacy-shell-version/` as a
fallback.

## Layout

- `dashboard/` — the app itself: `Cargo.toml`, `src/main.rs`,
  `ui/app.slint`, and icons. This code runs on the device.
- `slint-kindle-backend/` — a local, patched fork of
  [sverrejb/slint-kindle-backend](https://github.com/sverrejb/slint-kindle-backend)
  (MIT/Apache-2.0 license). The fork has three patches on top of the
  upstream code. All three patches are in `src/platform.rs`:
  1. **Touch made optional.** The upstream code crashes when it cannot
     open the touch device. On this Kindle, KOReader's reader process
     holds an exclusive lock on the touchscreen when KOReader is
     active. Because of this, touch input reliably fails unless
     KOReader stops first. This app does not stop KOReader. There is
     no reliable way to restart KOReader by remote if that fails.
     KOReader also provides SSH access to the device. The patch logs a
     warning and runs in display-only mode instead of a crash.
  2. **Always full refresh.** The upstream code sends a partial e-ink
     update for the changed area only. This partial update leaves
     faint ghosting from the prior screen content. The patch makes
     every refresh a full GC16 refresh instead.
  3. **Reset the stay-awake window after `on_wake`, not before.** The
     upstream suspend loop checks again for suspend on its next pass,
     before it polls or draws the screen. The old code started the
     stay-awake window right before it called `on_wake`. A callback
     slower than `stay_awake` (for example, an HTTP fetch during a
     network problem) then caused an immediate second suspend on the
     next loop check. In that case, the device never rendered the new
     data. The patch starts the stay-awake window after the callback
     returns instead.
- `legacy-shell-version/` — `kindle_weather.sh` (the FBInk/eips draw
  script) and `kindle_sleep_loop.sh` (its suspend/wake loop).
  `dashboard/` replaces this code. It stays in the repository as a
  working fallback.
- `scripts/make_washer_icon.py` / `scripts/make_trash_icon.py` — these
  scripts regenerate `dashboard/ui/icons/washer_done.svg` and
  `dashboard/ui/icons/trash.svg`, from Lucide's `washing-machine` and
  `trash-2` icons (ISC license). The weather icons come from
  [basmilius/meteocons](https://github.com/basmilius/meteocons) (MIT
  license, see `dashboard/ui/icons/LICENSE`). They use the
  `monochrome/svg-static` style, not the default colored `fill` style.
  The colored style renders as washed-out grey on this 8-bit-grayscale
  panel.

## Build

This build needs a Rust toolchain with the
`armv7-unknown-linux-musleabihf` target, and `cargo-zigbuild` (which
needs `zig`). This repository does not include these tools. Set them
up on your build machine:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild
# zig itself: apt/brew/apk install zig, or see ziglang.org
```

Then, from `dashboard/`, run:

```sh
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

The build creates one static binary:
`dashboard/target/armv7-unknown-linux-musleabihf/release/kindle_dashboard`.

## Deploy

```sh
scp -P 2222 target/armv7-unknown-linux-musleabihf/release/kindle_dashboard \
    root@<kindle-ip>:/mnt/us/extensions/weather_dashboard/kindle_dashboard_rs
ssh -p 2222 root@<kindle-ip> \
    "chmod +x /mnt/us/extensions/weather_dashboard/kindle_dashboard_rs"
```

By design, the device sleeps almost all the time (see Power, below).
Because of this, the SSH window for these commands is often only a few
seconds. Check that a deploy worked: compare the `md5sum` value on
both sides. Do not trust a silent success.

Two files must exist next to the binary, in
`/mnt/us/extensions/weather_dashboard/` on the device. This repository
does not include either file:
- `token.txt` — a Home Assistant long-lived access token. One line, no
  trailing newline. Run `chmod 600` on this file.
- `ha_url.txt` — the URL of your Home Assistant instance. One line,
  for example `http://10.0.0.5:8123`. If this file is missing, the app
  uses `http://homeassistant.local:8123` instead, so it still works by
  default against HA's default mDNS hostname.

Note: `/mnt/us` is the USB-visible partition. A future change can move
`token.txt` to `/var/local/` instead.

Launch:
```sh
cd /mnt/us/extensions/weather_dashboard && nohup ./kindle_dashboard_rs \
    > dashboard_stderr.log 2>&1 &
```

### KOReader coexistence

SSH access to the device comes from KOReader's built-in SSH server
(Settings → Network → SSH Server), on port 2222. This app does not
stop or replace KOReader. It writes directly to the framebuffer
alongside KOReader, and it runs without touch input, for the same
reason as the backend patch above.

To test something with KOReader's touch grab released: run
`killall -STOP awesome reader.lua`. This command pauses KOReader. It
does not stop it. `SIGSTOP` does not release KOReader's device locks,
but this dashboard does not need that. Run
`killall -CONT awesome reader.lua` to resume KOReader.

## Today's calendar items & alerts

The app reads `calendar.household` once for each refresh. This
calendar comes from Home Assistant's built-in Local Calendar
integration, which needs no external account. The app reads events for
the Kindle's own local day, not Home Assistant's server day. The app
calculates this day from its own clock, on purpose, to keep the
calendar day in sync with the date shown above it.

The app checks each all-day event's `summary` against a keyword table,
`ALERT_KEYWORDS` in `main.rs`. A match becomes an icon and a custom
message, in the bottom section. For example, `"trash"` becomes the
trash icon and the message `"Take out the trash!"`. An event with no
match becomes a plain line under the weather, in the top section. The
app never shows one matched event twice.

Alert matching checks all-day events only, on purpose. Every real
trash, recycling, or leaves reminder is an all-day event. This limit
stops the app from matching normal words in a timed event's title. For
example, a timed event titled "Flight leaves 6am" does not match
`"leaves"`. For a new alert keyword, pick a word that only makes
sense as an all-day chore or reminder. Do not pick a common verb that
can appear in a timed appointment's title.

To add a new alert:
1. Add a row to `ALERT_KEYWORDS` in `dashboard/src/main.rs`. Give the
   substring to match (not case-sensitive), an icon key, and the
   display message.
2. Add `dashboard/ui/icons/<key>.svg`. The easiest method: copy the
   pattern in an existing `scripts/make_<name>_icon.py` file.
3. Add one case to `alert-icon-for()` in `dashboard/ui/app.slint`. Map
   the icon key to that file.

No other code change is necessary. The fetch, the matching, and the
rendering are already generic over this table.

## Power / suspend

The backend's `set_wake_schedule` and `on_wake` API drives a real
suspend-to-RAM cycle. It uses `/sys/class/rtc/rtcN/wakealarm` and
`/sys/power/state`, not just a dimmed screen. Between refreshes, the
device is fully asleep, not idle and awake. The production interval is
10 minutes. `on_wake` fires immediately on resume, with no grace
period. WiFi needs a few seconds to reassociate first. See the
`thread::sleep` call in `refresh_after_wake()`, in `main.rs`.

## Screen orientation

The device is mounted rotated 90 degrees from normal portrait
orientation. The backend reads the real panel geometry from the
kernel, then scales its layout to that geometry automatically. The app
needs no manual rotation flag. The Kindle's own kernel framebuffer
rotation state is a separate, lower-level detail that the backend
already accounts for.
