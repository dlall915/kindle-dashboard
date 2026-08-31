#!/usr/bin/env python3
"""Lucide's circle-help icon (ISC license) for an alert whose keyword has
no matching case in alert-icon-for() in app.slint - a real, reachable
state (a new ALERT_KEYWORDS row in main.rs with no matching Slint case
added yet, audit finding #9), not a designed fallback. A question mark
reads as "unknown alert" at a glance, instead of misleadingly showing the
trash icon for a message that has nothing to do with trash.
"""

HELP_PATHS = """
<circle cx="12" cy="12" r="10" />
<path d="M9.09 9a3 3 0 0 1 5.83 1c0 2-3 3-3 3" />
<path d="M12 17h.01" />
"""

svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <g transform="translate(8,8) scale(3.5)" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    {HELP_PATHS}
  </g>
</svg>
"""

out = "/data/kindle-dashboard/dashboard/ui/icons/unknown.svg"
with open(out, "w") as f:
    f.write(svg)
print("wrote", out)
