#!/usr/bin/env python3
"""Lucide's leaf icon (ISC license) for the Kindle dashboard's
"Take out the leaves" calendar alert - same treatment as
make_washer_icon.py / make_trash_icon.py / make_recycling_icon.py: plain
icon, no badge.
"""

LEAF_PATHS = """
<path d="M11 20A7 7 0 0 1 9.8 6.1C15.5 5 17 4.48 19 2c1 2 2 4.18 2 8 0 5.5-4.78 10-11 10Z" />
<path d="M2 21c0-3 1.85-5.36 5.08-6C9.5 14.52 12 13 13 12" />
"""

svg = f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
  <g transform="translate(8,8) scale(3.5)" fill="none" stroke="#000" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">
    {LEAF_PATHS}
  </g>
</svg>
"""

out = "/data/kindle-dashboard/dashboard/ui/icons/leaves.svg"
with open(out, "w") as f:
    f.write(svg)
print("wrote", out)
