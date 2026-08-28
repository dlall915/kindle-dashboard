#!/bin/sh
# kindle_sleep_loop.sh — drives the Kindle's own suspend/wake cycle so the
# e-ink dashboard refreshes on a timer WITHOUT staying awake between draws.
# This is what actually delivers the battery-life goal; cron alone can't,
# since cron doesn't fire while the device is suspended.
#
# Run this once (nohup'd, backgrounded) — it loops forever: draw the
# dashboard, arm an RTC wake alarm, suspend to RAM, repeat when the alarm
# wakes it back up. Suspend-to-RAM keeps this process (and everything
# else — KOReader's SSH server, crond, etc.) alive in memory across each
# cycle, so no boot-time / init-level wiring is needed.
#
# Deploy: /mnt/us/extensions/weather_dashboard/kindle_sleep_loop.sh
# Start:  nohup /mnt/us/extensions/weather_dashboard/kindle_sleep_loop.sh \
#             >> /mnt/us/extensions/weather_dashboard/sleep_loop.log 2>&1 &
#
# This replaces the old cron-based refresh — do not run both at once, they
# will race and double-draw.

DIR="$(cd "$(dirname "$0")" && pwd)"
DRAW="$DIR/kindle_weather.sh"
INTERVAL=600   # seconds between refreshes (10 min). Tried 60s - WiFi needs
               # several seconds to reassociate after resume, and at 60s that
               # ate nearly the whole cycle: the weather fetch failed on
               # EVERY draw ("No response from Home Assistant"), and total
               # awake time per cycle was often under 10s, barely any real
               # sleep between cycles. Reverted.
RTC=/dev/rtc0
WAKEALARM=/sys/class/rtc/rtc0/wakealarm

while true; do
    # Set this BEFORE drawing, not after - drawing (especially with a full
    # flash) takes real time, and Amazon's own idle/screensaver timer keeps
    # running through it regardless of our own state. With this call after
    # the draw instead, the native screensaver could fire in that gap and
    # blank out content that had just drawn correctly.
    lipc-set-prop com.lab126.powerd preventScreenSaver 1 2>/dev/null

    echo "$(date '+%F %T') draw"
    "$DRAW"

    # BusyBox rtcwake reports "write error: Invalid argument" on this
    # device even when the alarm is set correctly (confirmed by reading
    # $WAKEALARM back) — its exit code/stderr can't be trusted, so verify
    # the register directly instead of trusting rtcwake's own status.
    rtcwake -d "$RTC" -m no -s "$INTERVAL" >/dev/null 2>&1

    NOW="$(date +%s)"
    ARMED="$(cat "$WAKEALARM" 2>/dev/null || echo 0)"
    if [ "$ARMED" -gt "$NOW" ]; then
        echo "$(date '+%F %T') alarm armed for $((ARMED - NOW))s, suspending"
        echo mem > /sys/power/state
        echo "$(date '+%F %T') resumed"
        # Give WiFi a moment to reassociate before the next loop iteration's
        # curl calls - confirmed via sleep_loop.log that drawing right on
        # resume with no wait fails the weather fetch outright.
        sleep 4
    else
        echo "$(date '+%F %T') WARNING: wake alarm not armed, staying awake and retrying next loop"
        sleep "$INTERVAL"
    fi
done
