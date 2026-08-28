# Kindle Dashboard

A native Rust + Slint dashboard for a Kindle Paperwhite 2 running
custom firmware, mounted as a low-power e-ink display for time,
weather, and a washing-machine-done alert, reading from a local Home
Assistant instance. Runs directly on the Kindle's framebuffer, no
browser, no X11.

Replaces an earlier shell-script + FBInk version (kept in
`legacy-shell-version/` for reference/fallback — it still works, just
superseded).

## Layout

- `dashboard/` — the app itself (Cargo.toml, `src/main.rs`, `ui/app.slint`,
  icons). This is what actually runs on the device.
- `slint-kindle-backend/` — a locally patched fork of
  [sverrejb/slint-kindle-backend](https://github.com/sverrejb/slint-kindle-backend)
  (MIT/Apache-2.0). Two patches on top of upstream, both in
  `src/platform.rs`:
  1. **Touch made optional.** Upstream crashes if the touch device
     can't be opened. On this device, KOReader's own reader process
     holds an exclusive lock on the touchscreen whenever it's the
     active app, so touch-open reliably fails unless KOReader is
     killed first (not attempted — no reliable remote way to relaunch
     KOReader if that went wrong, and it's what provides SSH access to
     the device in the first place). Patched to log a warning and run
     display-only instead of crashing.
  2. **Always full refresh.** Upstream only sends a partial e-ink
     update for whatever visibly changed, which leaves faint ghosting
     from prior screen content. Patched to always issue a full GC16
     refresh instead.
- `legacy-shell-version/` — `kindle_weather.sh` (FBInk/eips draw
  script) and `kindle_sleep_loop.sh` (the suspend/wake loop it runs
  under). Superseded by `dashboard/`, kept as a working fallback.
- `scripts/make_washer_icon.py` — regenerates
  `dashboard/ui/icons/washer_done.svg` from Lucide's `washing-machine`
  icon (ISC license). The weather icons themselves
  (`dashboard/ui/icons/*.svg`, except `washer_done.svg`) are from
  [basmilius/meteocons](https://github.com/basmilius/meteocons) (MIT —
  see `dashboard/ui/icons/LICENSE`), `monochrome/svg-static` style
  specifically (not the default colored `fill` style, which renders as
  washed-out grey on this 8-bit-grayscale panel).

## Build

Needs a Rust toolchain with the `armv7-unknown-linux-musleabihf`
target and `cargo-zigbuild` (which needs `zig`). None of this is
committed here — set it up fresh wherever you're building from:

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild
# zig itself: apt/brew/apk install zig, or see ziglang.org
```

Then, from `dashboard/`:

```sh
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

Output: `dashboard/target/armv7-unknown-linux-musleabihf/release/kindle_dashboard`
— a single static binary.

## Deploy

```sh
scp -P 2222 target/armv7-unknown-linux-musleabihf/release/kindle_dashboard \
    root@<kindle-ip>:/mnt/us/extensions/weather_dashboard/kindle_dashboard_rs
ssh -p 2222 root@<kindle-ip> \
    "chmod +x /mnt/us/extensions/weather_dashboard/kindle_dashboard_rs"
```

The device sleeps almost all the time by design (see Power below), so
the SSH window to actually run these commands in is often only a few
seconds — verify a deploy landed with `md5sum` on both sides rather
than trusting a silent success.

On the device, two files must exist alongside the binary in
`/mnt/us/extensions/weather_dashboard/`, neither committed here:
- `token.txt` — a Home Assistant long-lived access token, one line, no
  trailing newline, `chmod 600`.
- `ha_url.txt` — your Home Assistant instance's URL, one line, e.g.
  `http://10.0.0.5:8123`. Falls back to `http://homeassistant.local:8123`
  if this file is missing, so the app still runs out of the box against
  HA's default mDNS hostname.

(`token.txt` is worth moving to `/var/local/` instead of `/mnt/us` at
some point — that's the USB-visible partition.)

Launch:
```sh
cd /mnt/us/extensions/weather_dashboard && nohup ./kindle_dashboard_rs \
    > dashboard_stderr.log 2>&1 &
```

### KOReader coexistence

SSH access to the device comes from KOReader's own built-in SSH server
(Settings → Network → SSH Server), port 2222. This app doesn't kill or
replace KOReader — it just writes directly to the framebuffer
alongside it, and runs without touch input for the same reason (see
the backend patch notes above). If you need to test something with
KOReader's own touch grab released, `killall -STOP awesome reader.lua`
pauses it (not kill — `SIGSTOP` doesn't release its device locks, but
this dashboard doesn't need that anyway) and `killall -CONT awesome
reader.lua` resumes it.

## Power / suspend

The backend's `set_wake_schedule` / `on_wake` API drives a genuine
suspend-to-RAM cycle (`/sys/class/rtc/rtcN/wakealarm` +
`/sys/power/state`), not just a dimmed screen — the device is fully
asleep between refreshes, not idling awake. Production interval is 10
minutes. `on_wake` fires immediately on resume, not after any grace
period, and WiFi needs a few seconds to reassociate first — see the
explicit `thread::sleep` in `refresh_after_wake()` in `main.rs`.

## Screen orientation

Physically mounted rotated 90° from normal portrait. The backend reads
the real panel geometry from the kernel and scales its layout to it
automatically — no manual rotation flag needed at the app level (the
Kindle's own kernel framebuffer rotation state is a separate, lower
concern the backend already accounts for).
