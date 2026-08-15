#!/usr/bin/env python3
"""Walks the panel at phone width and reports what is no good there.

    scripts/mobile-check.py                   390 px, every page
    scripts/mobile-check.py --width 320       the narrowest device that still counts
    scripts/mobile-check.py --page console    one page only

Three things are measured: whether the page runs sideways (the coarse fault),
whether something sticks out of its box sideways without sitting in a scroll
area (the fine one), and whether tap targets are under 40 px — below that a
thumb no longer hits reliably.
"""
import argparse
import base64
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from drive import Browser, submit  # noqa: E402

IPHONE = (
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 "
    "(KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1"
)

# What is measured, and how to tell that the page is really there.
PAGES = [
    ("sign-in", "/", "!!document.querySelector('input[type=password]')"),
    ("list", "/", "!location.pathname.startsWith('/login')"),
    ("new", "/new", "document.body.innerText.length > 200"),
    ("overview", "/servers/{id}", "document.body.innerText.length > 200"),
    ("console", "/servers/{id}", "!!document.querySelector('.xterm, textarea, pre')"),
    ("files", "/servers/{id}/files", "document.body.innerText.length > 100"),
    ("content", "/servers/{id}/content", "document.body.innerText.length > 100"),
    ("browse", "/servers/{id}/browse", "document.body.innerText.length > 200"),
    ("backups", "/servers/{id}/backups", "document.body.innerText.length > 100"),
    ("access", "/servers/{id}/access", "document.body.innerText.length > 100"),
    ("options", "/servers/{id}/settings", "document.body.innerText.length > 100"),
    ("account", "/account", "document.body.innerText.length > 200"),
    ("users", "/admin/users", "document.body.innerText.length > 200"),
    ("settings", "/admin/settings", "document.body.innerText.length > 100"),
    ("addresses", "/admin/playit", "document.body.innerText.length > 100"),
    ("applications", "/admin/registrations", "document.body.innerText.length > 100"),
    ("mail", "/admin/mail", "document.body.innerText.length > 100"),
    ("drives", "/admin/drive", "document.body.innerText.length > 100"),
    # There is no line here for the Drive card of one's own account: it is a card in
    # `/account` (`pages/account/sections.ts`) and not a route, so "account" measures it too.

    # The three session-free pages that leave a signed-in visitor standing as well (they redeem
    # a token, `pages/auth/routes.ts`). The two forms are in OPEN_PAGES, because a signed-in
    # visitor is sent away from them.
    ("verify", "/verify-email", "document.body.innerText.length > 40"),
    ("pending", "/registration-pending", "document.body.innerText.length > 40"),
    ("new-password", "/reset-password", "document.body.innerText.length > 40"),
]

# Measured before signing in, because the guard sends a signed-in visitor from these two forms to
# the server list (`whenSignedIn: 'bounce'`). Measured after signing in, this would be the server
# list three times over, and that one passes every check.
OPEN_PAGES = [
    ("register", "/register", "!!document.querySelector('input[type=email]')"),
    ("forgot-password", "/forgot-password", "!!document.querySelector('input[type=email]')"),
]

# Measured against the device width that is handed in from outside — not against
# `window.innerWidth`. If a page runs far over, the browser blows the layout viewport up to the
# content width itself; then `innerWidth` is a yardstick cut from the very content it measures and
# every page passes. That is exactly how the first run reported 13 clean pages while half of the
# tabs lay outside the picture.
MEASURE = r"""((device) => {
  const name = (e) => {
    const cls = typeof e.className === 'string' && e.className
      ? '.' + e.className.trim().split(/\s+/).slice(0, 3).join('.') : '';
    return e.tagName.toLowerCase() + cls;
  };
  const scrollbar = (e) => {
    const s = getComputedStyle(e);
    return /auto|scroll/.test(s.overflowX) || /auto|scroll/.test(s.overflow);
  };
  const inScroller = (e) => {
    for (let p = e.parentElement; p && p !== document.body; p = p.parentElement) {
      if (scrollbar(p)) return true;
    }
    return false;
  };

  const all = [...document.querySelectorAll('body *')];
  const visible = all.filter(e => {
    const r = e.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) return false;
    const s = getComputedStyle(e);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  });

  const sticking_out = visible
    .filter(e => e.getBoundingClientRect().right > device + 2 && !inScroller(e))
    .map(e => ({ el: name(e), right: Math.round(e.getBoundingClientRect().right) }));

  const swiping = visible
    .filter(e => scrollbar(e) && e.scrollWidth > e.clientWidth + 8)
    .map(e => ({ el: name(e), wide: e.scrollWidth, box: e.clientWidth }));

  // Links in the middle of a sentence do not count: those are type, not a button, and
  // at 40 px tall they would be a foreign body in the paragraph.
  const clickable = 'button, a[href], [role=button], input:not([type=hidden]), select, summary';
  const small = visible
    .filter(e => e.matches(clickable))
    .filter(e => !e.closest('[aria-hidden=true]'))
    .filter(e => !(e.tagName === 'A' && getComputedStyle(e).display.startsWith('inline')))
    .map(e => ({ el: name(e), text: (e.innerText || e.value || '').trim().slice(0, 24),
                 h: Math.round(e.getBoundingClientRect().height),
                 w: Math.round(e.getBoundingClientRect().width) }))
    .filter(t => t.h > 0 && (t.h < 40 || t.w < 24));

  const small_type = visible
    .filter(e => e.children.length === 0 && (e.innerText || '').trim().length > 10)
    .map(e => ({ el: name(e), px: parseFloat(getComputedStyle(e).fontSize) }))
    .filter(t => t.px < 12);

  const dedupe = (list) => {
    const seen = new Map();
    for (const t of list) if (!seen.has(t.el)) seen.set(t.el, t);
    return [...seen.values()];
  };

  return JSON.stringify({
    width: device,
    inflated: window.innerWidth > device ? window.innerWidth : 0,
    sideways: document.documentElement.scrollWidth - device,
    sticking_out: dedupe(sticking_out).slice(0, 8),
    swiping: dedupe(swiping).slice(0, 6),
    small: dedupe(small).slice(0, 8),
    small_type: dedupe(small_type).slice(0, 4),
  });
})"""


def measure(b, args, report, name):
    raw = json.loads(b.js(f"({MEASURE})({args.width})"))
    report[name] = raw
    shot = b.call("Page.captureScreenshot", format="png")["data"]
    with open(f"{args.images}/{args.width}-{name}.png", "wb") as f:
        f.write(base64.b64decode(shot))

    faults = []
    if raw["sideways"] > 2:
        faults.append(f"page runs {raw['sideways']} px sideways")
    if raw["inflated"]:
        faults.append(f"viewport inflated to {raw['inflated']} px")
    if raw["sticking_out"]:
        faults.append(f"{len(raw['sticking_out'])}× sticks out")
    if raw["swiping"]:
        faults.append(f"{len(raw['swiping'])}× has to be swiped")
    if raw["small"]:
        faults.append(f"{len(raw['small'])}× tap target too small")
    if raw["small_type"]:
        faults.append(f"{len(raw['small_type'])}× type under 12 px")

    mark = "\033[31m✗\033[0m" if faults else "\033[32m✓\033[0m"
    print(f"{mark} {name:14s} {', '.join(faults) if faults else 'clean'}")
    for key in ("sticking_out", "swiping", "small", "small_type"):
        for t in raw[key]:
            print(f"      {key:12s} {t}")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--width", type=int, default=390)
    p.add_argument("--height", type=int, default=844)
    p.add_argument("--base", default="http://127.0.0.1:8099")
    p.add_argument("--server", default=os.environ.get("CRAFTPANEL_SERVER", ""))
    p.add_argument("--user", default="checker")
    p.add_argument("--password", default=os.environ.get("CRAFTPANEL_PW", ""))
    p.add_argument("--page", action="append", default=[])
    p.add_argument("--images", default="/tmp/mobile")
    args = p.parse_args()

    if not args.password:
        sys.exit("no password: set CRAFTPANEL_PW or pass --password")

    os.makedirs(args.images, exist_ok=True)
    chosen = [s for s in PAGES if not args.page or s[0] in args.page]
    open_pages = [s for s in OPEN_PAGES if not args.page or s[0] in args.page]
    report = {}

    b = Browser(port=9512)
    try:
        b.call("Emulation.setDeviceMetricsOverride", width=args.width, height=args.height,
               deviceScaleFactor=2, mobile=True)
        b.call("Emulation.setTouchEmulationEnabled", enabled=True, maxTouchPoints=5)
        b.call("Emulation.setUserAgentOverride", userAgent=IPHONE)

        b.goto(args.base)
        b.wait_for("!!document.querySelector('input[type=password]')", 30)

        # Signing in stands before the loop and not inside one of its steps. Hung on "list",
        # `--page console` would in truth have checked the sign-in mask thirteen times over and
        # reported it as clean.
        if any(name == "sign-in" for name, _, _ in chosen):
            measure(b, args, report, "sign-in")

        for name, path, ready in open_pages:
            b.goto(args.base + path)
            b.wait_for(ready, 20)
            b.settle(2)
            # Not released means: the guard threw us onto the sign-in page, and that one passes
            # every check. That is the house fault, and here it shows.
            if b.js("location.pathname.startsWith('/login')"):
                print(f"\033[31m✗\033[0m {name:14s} thrown onto the sign-in page — not released")
                continue
            measure(b, args, report, name)

        b.goto(args.base)
        b.wait_for("!!document.querySelector('input[type=password]')", 30)
        submit(b, args.user, args.password)
        b.wait_for("!location.pathname.startsWith('/login')", 30)

        for name, path, ready in chosen:
            if name == "sign-in":
                continue
            if "{id}" in path and not args.server:
                continue
            b.goto(args.base + path.replace("{id}", args.server))
            b.wait_for(ready, 20)
            b.settle(2)
            # An expired session throws us onto the sign-in page, and that one passes every check.
            if b.js("location.pathname.startsWith('/login')"):
                print(f"\033[31m✗\033[0m {name:14s} thrown onto the sign-in page — not measured")
                continue
            measure(b, args, report, name)
    finally:
        b.close()

    with open(f"{args.images}/{args.width}-report.json", "w") as f:
        json.dump(report, f, indent=2)
    print(f"\nImages and report: {args.images}/{args.width}-*")


if __name__ == "__main__":
    main()
