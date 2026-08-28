#!/bin/sh
# kindle_weather.sh — draws time + date + weather to the Kindle's e-ink
# screen via FBInk, reading current data from this Home Assistant
# instance's REST API.
#
# Deploy layout (adjust to match what WinterBreak2 actually set up):
#   /mnt/us/extensions/weather_dashboard/kindle_weather.sh   <- this file
#   /mnt/us/extensions/weather_dashboard/token.txt           <- long-lived
#       access token, ONE line, no trailing newline, chmod 600. Never
#       committed anywhere — lives only on the Kindle.
#
# Run this on a timer (cron/KUAL, every 10-15 min) — see the deploy
# instructions for how to register that on your specific jailbreak.

DIR="$(cd "$(dirname "$0")" && pwd)"
TOKEN_FILE="$DIR/token.txt"
ICON_DIR="$DIR/icons"
# HA_URL_FILE, one line, e.g. http://10.0.0.5:8123 - keeps a real
# deployment's actual LAN address out of source control.
HA_URL_FILE="$DIR/ha_url.txt"
HA_URL="$(cat "$HA_URL_FILE" 2>/dev/null || echo "http://homeassistant.local:8123")"
# KOReader bundles its own fbink build — use that directly rather than
# relying on fbink being in $PATH (it isn't, on this device).
FBINK="/mnt/us/koreader/fbink"
FONT="IBM"
# This FBInk build can't draw images (confirmed via its own --help
# capability line: "Image=No"). The stock Amazon `eips` tool can, so it
# draws the weather icon while FBInk handles all the text.
EIPS="/usr/sbin/eips"

# The Kindle is mounted rotated 90° from normal portrait. FBInk itself has
# no rotation flag, but the kernel framebuffer driver exposes one via
# sysfs. Setting it to 0 here (every run, not just once) is deliberate:
# this value does not survive every sleep/wake cycle, and KOReader's own
# UI is unaffected by it either way, so it's cheap and safe to reassert
# on each draw rather than trust it to stick.
echo 0 > /sys/class/graphics/fb0/rotate 2>/dev/null

if [ ! -r "$TOKEN_FILE" ]; then
    echo "Missing $TOKEN_FILE — paste your long-lived access token there first." >&2
    exit 1
fi
TOKEN="$(cat "$TOKEN_FILE")"

# --- Ghosting cleanup: full flashing refresh on every run. A partial
# refresh every 6th run was tried first, but left visible ghosting
# (faint horizontal banding) between flashes, so every draw flashes now. ---
FLASH_FLAG="-f"

# --- Washing machine done alert: takes over the whole screen (no time/
# date/weather) whenever the helper is on. It clears itself back to the
# normal dashboard once the next wash cycle starts (the automation that
# owns this helper turns it off at that point, not on any timer here). ---
WASHER_JSON="$(curl -s -m 10 -H "Authorization: Bearer $TOKEN" \
    "$HA_URL/api/states/input_boolean.basement_washing_machine_done")"
WASHER_DONE="$(echo "$WASHER_JSON" | sed -n 's/.*"state":"\([^"]*\)".*/\1/p' | head -1)"

if [ "$WASHER_DONE" = "on" ]; then
    "$FBINK" -q $FLASH_FLAG -k
    ALERT_ICON="$ICON_DIR/washer_done.png"
    ALERT_ICON_W=400
    ALERT_ICON_Y=90
    if [ -r "$ALERT_ICON" ]; then
        "$EIPS" -g "$ALERT_ICON" -x $(( (1024 - ALERT_ICON_W) / 2 )) -y $ALERT_ICON_Y >/dev/null 2>&1
    fi
    "$FBINK" -q -F "$FONT" -y 0 -Y $((ALERT_ICON_Y + ALERT_ICON_W + 40)) -S 4 -m "Washing Machine Done"
    exit 0
fi

# --- Fetch weather (temperature + condition) ---
WEATHER_JSON="$(curl -s -m 10 -H "Authorization: Bearer $TOKEN" \
    "$HA_URL/api/states/weather.home")"

if [ -z "$WEATHER_JSON" ]; then
    echo "No response from Home Assistant — check WiFi / HA_URL." >&2
    CONDITION="(offline)"
    TEMP=""
else
    CONDITION="$(echo "$WEATHER_JSON" | sed -n 's/.*"state":"\([^"]*\)".*/\1/p' | head -1)"
    TEMP="$(echo "$WEATHER_JSON" | sed -n 's/.*"temperature":\([0-9.]*\).*/\1/p' | head -1)"
fi

# %-I / %-d (no leading zero) are GNU-only date extensions — the
# Kindle's busybox date almost certainly doesn't support them, so
# stick to the portable zero-padded forms.
TIME_STR="$(date '+%I:%M %p')"
DATE_STR="$(date '+%A, %B %d')"
if [ -n "$TEMP" ]; then
    WEATHER_STR="${TEMP}°F, ${CONDITION}"
else
    WEATHER_STR="${CONDITION}"
fi

# --- Map the HA weather condition to an icon file. HA's standard
# condition set has more values than are worth drawing separate icons
# for, so several share one (rainy/pouring stay distinct; the rest
# group by rough visual similarity). Falls back to a plain cloud for
# anything unrecognized (including the offline placeholder). ---
case "$CONDITION" in
    sunny) ICON="sunny" ;;
    clear-night) ICON="clear-night" ;;
    partlycloudy) ICON="partlycloudy" ;;
    cloudy) ICON="cloudy" ;;
    rainy) ICON="rainy" ;;
    pouring) ICON="pouring" ;;
    lightning|lightning-rainy) ICON="lightning" ;;
    snowy|snowy-rainy) ICON="snowy" ;;
    fog|exceptional) ICON="fog" ;;
    windy|windy-variant) ICON="windy" ;;
    hail) ICON="hail" ;;
    *) ICON="default" ;;
esac
ICON_FILE="$ICON_DIR/$ICON.png"

# --- Draw ---
# Clear the whole screen first - FBInk only touches the pixels it's told
# to draw, so without this, anything that was on screen before (a KOReader
# menu, old content from switching apps) stays visible around our text.
# Flash here (not on a later call) so the ghosting cleanup covers the icon
# too, not just the text.
"$FBINK" -q $FLASH_FLAG -k

"$FBINK" -q -F "$FONT" -y 1 -S 10 -m "$TIME_STR"
"$FBINK" -q -F "$FONT" -y 12 -S 3 -m "$DATE_STR"

# The icon is centered independently by pixel position (eips takes raw
# x/y, not FBInk's row/col grid) rather than as one block with the text
# below it - simpler than computing the combined width of a string
# whose length changes every run. The date line's actual rendered
# height ran taller than its nominal row/scale math suggested (it
# overlapped an icon placed right after it), so this gap is generous
# on purpose rather than tightly computed.
ICON_W=190
ICON_Y=360
if [ -r "$ICON_FILE" ]; then
    "$EIPS" -g "$ICON_FILE" -x $(( (1024 - ICON_W) / 2 )) -y $ICON_Y >/dev/null 2>&1
fi
# -y (row) landed the weather text at the very top instead of below the
# icon - FBInk's row grid apparently shrinks at higher -S values (fewer
# rows fit at a bigger scale), and this row number was past whatever
# that scale's valid range is. -Y is a raw pixel offset instead of a
# row, so it isn't subject to that ambiguity - used here for reliable
# placement below the icon.
"$FBINK" -q -F "$FONT" -y 0 -Y $((ICON_Y + ICON_W + 20)) -S 3 -m "$WEATHER_STR"

# --- Battery percentage, small, bottom-right corner ---
# Negative -x/-y (count back from an edge) made FBInk stack the
# characters vertically instead of printing them in a row - it seems
# to compute almost no horizontal room that close to the edge and wraps
# each character onto its own line. -X/-Y are plain pixel nudges applied
# after normal layout, so they don't trigger that wrap check.
BATT="$(lipc-get-prop com.lab126.powerd battLevel 2>/dev/null)"
if [ -n "$BATT" ]; then
    "$FBINK" -q -F "$FONT" -x 0 -y 0 -X 900 -Y 700 -S 2 "${BATT}%"
fi
