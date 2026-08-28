#!/usr/bin/env python3
"""Lucide's washing-machine icon (ISC license) for the Kindle dashboard's
washer-done alert - no badge, just the plain icon; "Done!" is already
conveyed by the text underneath it.
"""

WASHER_PATHS = """
<path d="M3 6h3" />
<path d="M17 6h.01" />
<rect width="18" height="20" x="3" y="2" rx="2" />
<circle cx="12" cy="13" r="5" />
<path d="M12 18a2.5 2.5 0 0 0 0-5 2.5 2.5 0 0 1 0-5" />
"""

svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <g transform="translate(8,8) scale(3.5)" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    {WASHER_PATHS}
  </g>
</svg>
"""

out = "/tmp/claude-0/-homeassistant/48cd087d-db8f-4c80-8dda-98be3ef884f5/scratchpad/kindle-slint-spike/ui/icons/washer_done.svg"
with open(out, "w") as f:
    f.write(svg)
print("wrote", out)
