"""Drives a real Chromium over the DevTools protocol, without any dependency.

There is no pip on the machines this runs on, so the WebSocket framing and the
command plumbing are here rather than in a library.
"""

import base64
import json
import os
import re
import socket
import struct
import subprocess
import sys
import time
import urllib.request

BASE = os.environ.get("CRAFTPANEL_BASE", "http://127.0.0.1:8099")
PASSWORD = os.environ["CRAFTPANEL_PW"]
SHOTS = os.environ.get("CRAFTPANEL_SHOT", "/tmp/craftpanel-acceptance")
CHROME = next(
    (
        p
        for p in subprocess.run(
            ["bash", "-c", "ls -d /root/.cache/ms-playwright/chromium-*/chrome-linux*/chrome"],
            capture_output=True, text=True,
        ).stdout.split()
    ),
    None,
)


class Browser:
    def __init__(self, port=9333):
        self.port = port
        self.proc = subprocess.Popen(
            [CHROME, "--headless=new", "--no-sandbox", "--disable-gpu",
             "--disable-dev-shm-usage", f"--remote-debugging-port={port}",
             "--window-size=1440,900", "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        self.sock = None
        self.next_id = 0
        self._connect()

    def _connect(self):
        for _ in range(60):
            try:
                pages = json.load(urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json"))
                target = next(p for p in pages if p["type"] == "page")
                url = target["webSocketDebuggerUrl"]
                break
            except Exception:
                time.sleep(0.5)
        else:
            raise RuntimeError("chromium never opened its debugging port")

        host, rest = url.split("://", 1)[1].split("/", 1)
        h, p = host.split(":")
        self.sock = socket.create_connection((h, int(p)))
        self.sock.settimeout(30)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(
            f"GET /{rest} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\n"
            f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\n"
            f"Sec-WebSocket-Version: 13\r\n\r\n".encode()
        )
        self.buf = b""
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.sock.recv(4096)
        self.buf = head.split(b"\r\n\r\n", 1)[1]

    def _send(self, payload):
        data = json.dumps(payload).encode()
        mask = os.urandom(4)
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        n = len(data)
        if n < 126:
            header = struct.pack("!BB", 0x81, 0x80 | n)
        elif n < 65536:
            header = struct.pack("!BBH", 0x81, 0x80 | 126, n)
        else:
            header = struct.pack("!BBQ", 0x81, 0x80 | 127, n)
        self.sock.sendall(header + mask + masked)

    def _frames(self):
        while True:
            while len(self.buf) >= 2:
                b1, b2 = self.buf[0], self.buf[1]
                length = b2 & 0x7F
                i = 2
                if length == 126:
                    length = struct.unpack("!H", self.buf[i:i + 2])[0]; i += 2
                elif length == 127:
                    length = struct.unpack("!Q", self.buf[i:i + 8])[0]; i += 8
                if len(self.buf) < i + length:
                    break
                payload = self.buf[i:i + length]
                self.buf = self.buf[i + length:]
                if b1 & 0x0F == 1:
                    yield json.loads(payload.decode())
            chunk = self.sock.recv(1 << 20)
            if not chunk:
                raise RuntimeError("chromium closed the connection")
            self.buf += chunk

    def call(self, method, **params):
        self.next_id += 1
        wanted = self.next_id
        self._send({"id": wanted, "method": method, "params": params})
        for message in self._frames():
            if message.get("id") == wanted:
                if "error" in message:
                    raise RuntimeError(f"{method}: {message['error']}")
                return message.get("result", {})

    def goto(self, url):
        self.call("Page.enable")
        self.call("Page.navigate", url=url)
        self.settle()

    def js(self, expression):
        result = self.call(
            "Runtime.evaluate", expression=expression, awaitPromise=True, returnByValue=True
        )
        if result.get("exceptionDetails"):
            raise RuntimeError(result["exceptionDetails"].get("text", "script failed"))
        return result.get("result", {}).get("value")

    def settle(self, seconds=1.5):
        time.sleep(seconds)

    def wait_for(self, expression, timeout=45, every=0.5):
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                if self.js(expression):
                    return True
            except Exception:
                pass
            time.sleep(every)
        return False

    def shoot(self, name):
        data = self.call("Page.captureScreenshot", format="png")["data"]
        path = os.path.join(SHOTS, f"{name}.png")
        with open(path, "wb") as handle:
            handle.write(base64.b64decode(data))
        return path

    def close(self):
        try:
            self.sock.close()
        finally:
            self.proc.terminate()


# Vue reacts on the next tick, so a submit button guarded by "both fields filled"
# is still disabled the instant after the input event. Wait for it rather than
# reading the state we just caused.
FILL = """
(async () => {
  const set = (el, value) => {
    const setter = Object.getOwnPropertyDescriptor(el.constructor.prototype, 'value').set;
    setter.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };
  const inputs = [...document.querySelectorAll('input')];
  const passwords = inputs.filter(i => i.type === 'password');
  if (!passwords.length) return 'no password field';
  if (%USER%) {
    const user = inputs.find(i => /user|name/i.test(i.name + i.id + (i.getAttribute('autocomplete')||'')));
    if (!user) return 'no username field';
    set(user, %USER%);
  }
  // The change-password form asks for the old one and the new one twice.
  // `autocomplete` tells them apart more reliably than order does.
  for (const field of passwords) {
    const wants = field.getAttribute('autocomplete') === 'current-password' && %OLD% !== null;
    set(field, wants ? %OLD% : %PASS%);
  }

  const wait = ms => new Promise(r => setTimeout(r, ms));
  let button = null;
  for (let i = 0; i < 20 && !button; i++) {
    await wait(100);
    button = [...document.querySelectorAll('button[type=submit], form button')].find(b => !b.disabled);
  }
  if (!button) {
    const form = document.querySelector('form');
    if (!form) return 'no form and no button';
    form.requestSubmit();
    return 'submitted';
  }
  button.click();
  return 'submitted';
})()
"""

def submit(browser, user, password, old=None):
    """Fills the form on screen and presses its button. `old` is the current
    password, which only the change-password form asks for."""
    script = (
        FILL.replace("%USER%", json.dumps(user) if user else "null")
        .replace("%PASS%", json.dumps(password))
        .replace("%OLD%", json.dumps(old) if old else "null")
    )
    outcome = browser.js(script)
    step("form submitted", outcome == "submitted", str(outcome))
    return outcome == "submitted"


steps = []


def step(name, passed, note=""):
    steps.append((name, passed, note))
    mark = "\033[32mok \033[0m" if passed else "\033[31mNO \033[0m"
    print(f"  {mark} {name}{(' — ' + note) if note else ''}", flush=True)


def main():
    if not CHROME:
        print("  no chromium found")
        return 1

    os.makedirs(SHOTS, exist_ok=True)
    browser = Browser()
    try:
        browser.goto(BASE)
        # `document.title` is defined on Chromium's own error page too, so ask
        # something only our own document can answer.
        served = browser.js(
            "!!document.getElementById('app') && "
            "!!document.querySelector('script[src*=\"/assets/\"]')"
        )
        step("panel served the interface", bool(served))
        if not served:
            step("reachable at all", False, browser.js("document.body.innerText.slice(0,120)") or "no body")

        reached_login = browser.wait_for(
            "!!document.querySelector('input[type=password]')", timeout=30
        )
        step("login screen appears", reached_login)
        browser.shoot("1-login")

        if not reached_login:
            return 1

        submit(browser, "operator", PASSWORD)
        left_login = browser.wait_for("!location.pathname.startsWith('/login')", timeout=30)
        step("signed in", left_login, browser.js("location.pathname") or "?")
        browser.settle(2)
        browser.shoot("2-after-login")

        # A generated password must be changed before anything else is reachable,
        # so the first stop after signing in is that form, not the server list.
        if (browser.js("location.pathname") or "").startswith("/change-password"):
            new_password = "Testrun-" + base64.b32encode(os.urandom(10)).decode().lower()
            submit(browser, None, new_password, old=PASSWORD)
            done = browser.wait_for(
                "!location.pathname.startsWith('/change-password')", timeout=30
            )
            step("forced password change accepted", done, browser.js("location.pathname") or "?")
            browser.settle(2)

        step("landed on the server list", (browser.js("location.pathname") or "") == "/")
        browser.shoot("3-server-list")

        body = browser.js("document.body.innerText.slice(0, 4000)") or ""
        step("interface rendered", len(body.strip()) > 40, f"{len(body)} chars")

        styled = browser.js("getComputedStyle(document.body).backgroundColor")
        step("theme applied", styled not in (None, "", "rgba(0, 0, 0, 0)"), str(styled))

        step(
            "shows an empty server list rather than an error",
            bool(browser.js("!/error|failed|went wrong/i.test(document.body.innerText)")),
        )
        step(
            "the administrator's system account is ready",
            bool(browser.js("!/system account is not ready/i.test(document.body.innerText)")),
        )

        for name, path in (("4-new-server", "/new"), ("5-admin-users", "/admin/users")):
            browser.goto(f"{BASE}{path}")
            browser.settle(2)
            reached = (browser.js("location.pathname") or "") == path
            text = browser.js("document.body.innerText.length") or 0
            step(f"{path} reachable", reached and text > 40, f"{text} chars")
            browser.shoot(name)

        print()
        for name, passed, _ in steps:
            if not passed:
                print(f"  failing step: {name}")
        return 0 if all(p for _, p, _ in steps) else 1
    finally:
        browser.close()


if __name__ == "__main__":
    sys.exit(main())
