"""Fails when an agent hippo knows would render without its own mark.

    python3 -I docs/site/check-agent-marks.py <repository root>

The agents hippo knows are `KNOWN` in crates/hippo-view/src/night/mod.rs
(code, tool, name, mark file). For each one this checks:

- brand/agents/<file>.svg exists, and Hippo.app's copy
  (app/hippo-app/icons/agent-<file>.svg) is the same file;
- the live demo (docs/site/demo/demo.js) draws its code with exactly that
  mark from one table assigned once, and knows no code hippo does not;
- every agent mark on the landing page (`<i class="ag" title="<name>">`)
  of a known agent is exactly that mark;
- the demo's data (docs/site/demo/data.js) names only known agents or
  `other`, with their names, and has no letter form (`short`) anywhere.

build.sh runs it; so does CI through build.sh.
"""

import json
import re
import sys
from pathlib import Path

root = Path(sys.argv[1] if len(sys.argv) > 1 else ".")
errors = []


def fail(msg):
    errors.append(msg)


def inner(svg):
    """What is between the <svg ...> tag and </svg>, spaces squeezed."""
    m = re.search(r"<svg[^>]*>(.*)</svg>", svg, re.S)
    return re.sub(r"\s+", " ", m.group(1)).strip() if m else None


src = (root / "crates/hippo-view/src/night/mod.rs").read_text()
block = re.search(r"pub const KNOWN[^=]*=\s*\[(.*?)\];", src, re.S)
known = re.findall(r'\("([^"]+)",\s*"([^"]+)",\s*"([^"]+)",\s*"([^"]+)"\)', block.group(1)) if block else []
if len(known) < 6:
    fail(f"could not read KNOWN from hippo-view (found {len(known)} agents)")
by_code = {code: (name, stem) for code, _tool, name, stem in known}
by_name = {name: stem for _code, _tool, name, stem in known}

marks = {}
for code, _tool, name, stem in known:
    brand = root / "brand/agents" / f"{stem}.svg"
    if not brand.is_file():
        fail(f"{name}: no mark at {brand.relative_to(root)}")
        continue
    text = brand.read_text()
    marks[stem] = inner(text)
    if not marks[stem]:
        fail(f"{name}: {brand.relative_to(root)} has no drawing")
    app = root / "app/hippo-app/icons" / f"agent-{stem}.svg"
    if not app.is_file() or app.read_bytes() != brand.read_bytes():
        fail(f"{name}: {app.relative_to(root)} is not the same file as {brand.relative_to(root)}")

# The live demo's marks.
demo = (root / "docs/site/demo/demo.js").read_text()
mblock = re.search(r"var MARKS = \{(.*?)\n\s*\};", demo, re.S)
# The table is assigned once: in 0.7.1 it was `var M`, and a later
# `var M = D.map` in the same function replaced it, so every agent drew a
# terminal.
if len(re.findall(r"\bMARKS\s*=(?!=)", demo)) != 1:
    fail("demo.js: the agent mark table (MARKS) is assigned more than once")
demo_marks = dict(re.findall(r"^\s*(\w+): '(.*)',?$", mblock.group(1), re.M)) if mblock else {}
if not demo_marks:
    fail("demo.js: no agent marks (var MARKS = { ... })")
for code, (name, stem) in by_code.items():
    got = demo_marks.get(code)
    if got is None:
        fail(f"demo.js: {name} ({code}) has no mark, so it would draw a terminal")
    elif re.sub(r"\s+", " ", got).strip() != marks.get(stem):
        fail(f"demo.js: {name} ({code}) is not drawn with brand/agents/{stem}.svg")
for code in demo_marks:
    if code not in by_code:
        fail(f"demo.js: a mark for {code}, an agent hippo does not know")
if re.search(r"\.short\b", demo):
    fail("demo.js: reads an agent's letter form (.short)")

# The landing page's marks.
landing = (root / "docs/site/landing.html").read_text()
seen = 0
for title, svg in re.findall(r'<i class="ag" title="([^"]+)">(<svg.*?</svg>)</i>', landing, re.S):
    if title in by_name:
        seen += 1
        if inner(svg) != marks.get(by_name[title]):
            fail(f"landing.html: {title} is not drawn with brand/agents/{by_name[title]}.svg")
if landing.count('class="ag"') and not seen:
    fail("landing.html: agent marks found but none could be read")

# The demo's data.
data = (root / "docs/site/demo/data.js").read_text()
if '"short"' in data:
    fail('data.js: an agent has a letter form ("short"); regenerate with demo-data.sh')
agents = re.findall(r'"agent":(\{[^{}]*\})', data)
if not agents:
    fail("data.js: no agent found")
for raw in agents:
    a = json.loads(raw)
    code = a.get("code")
    if code == "other":
        continue
    if code not in by_code:
        fail(f"data.js: agent code {code!r} is neither known nor 'other'")
    elif a.get("name") != by_code[code][0]:
        fail(f"data.js: {code} is named {a.get('name')!r}, not {by_code[code][0]!r}")

if errors:
    for e in errors:
        print(f"error: {e}", file=sys.stderr)
    sys.exit(1)
print(f"agent marks: {len(known)} known agents, each drawn with its own mark in brand/, the app, the demo ({len(agents)} agents in its data) and the landing page ({seen} marks)")
