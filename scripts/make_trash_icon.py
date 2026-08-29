#!/usr/bin/env python3
"""Lucide's trash-2 icon (ISC license) for the Kindle dashboard's
"Take out the trash" calendar alert - same treatment as make_washer_icon.py:
plain icon, no badge, the message text underneath already says what's
needed.
"""

TRASH_PATHS = """
<path d="M3 6h18" />
<path d="M19 6v14c0 1-1 2-2 2H7c-1 0-2-1-2-2V6" />
<path d="M8 6V4c0-1 1-2 2-2h4c1 0 2 1 2 2v2" />
<line x1="10" x2="10" y1="11" y2="17" />
<line x1="14" x2="14" y1="11" y2="17" />
"""

svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <g transform="translate(8,8) scale(3.5)" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    {TRASH_PATHS}
  </g>
</svg>
"""

out = "/data/kindle-dashboard/dashboard/ui/icons/trash.svg"
with open(out, "w") as f:
    f.write(svg)
print("wrote", out)
