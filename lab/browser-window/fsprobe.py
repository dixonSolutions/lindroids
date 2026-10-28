#!/usr/bin/env python3
"""Full screen without a tap: what a page in a Custom Tab can get, and when.

Run inside Termux: `python fsprobe.py 48200`. `/p/<name>?<variant flags>` is a page that tries
full screen as early as it can and reports, to ~/fsprobe.log, what it saw: user activation at
load, each `requestFullscreen` and its result, the viewport against the screen (the browser's bar
shows as `screen - inner`), every resize. Flags (any combination):

  load      requestFullscreen when the script runs, on DOMContentLoaded, load, rAF, 0/500/2000 ms
  scroll    the page is taller than the window; it scrolls itself (scrollTo, scrollIntoView, a
            fragment) to see whether the browser hides its bar for a scroll it did not get from
            a finger
  tall      the page is taller than the window, nothing else (for a finger scroll by `input swipe`)
  events    every touch/pointer/mouse/key event tries full screen; the log says which one got it
  manifest  links a manifest with `display: fullscreen` and `display_override`
  theme     `theme-color` and `viewport-fit=cover`

`curl 127.0.0.1:48200/state/<name>` prints the page's last report (the lab script polls it).
"""
import json, os, sys, time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 48200
LOG = os.path.expanduser("~/fsprobe.log")
T0 = time.time()
STATE = {}


def log(*a):
    line = "%8.2f %s" % (time.time() - T0, " ".join(str(x) for x in a))
    print(line, flush=True)
    with open(LOG, "a") as f:
        f.write(line + "\n")


PAGE = r"""<!doctype html><html><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1VIEWPORT">
THEME MANIFEST<title>fs NAME</title>
<style>html,body{margin:0;background:#0f1720;color:#fff;font:16px sans-serif}
#s{position:fixed;top:0;left:0;right:0;padding:8px;white-space:pre-wrap}
.tall{height:300vh}#end{position:absolute;bottom:0}</style></head>
<body class="TALL"><div id=s></div><div id=end>end</div>
<script>
const name = "NAME", flags = new Set(location.search.slice(1).split('&').map(s => s.split('=')[0]));
const t0 = performance.now();
let seq = 0, got = null;
function geo() {
  const vv = window.visualViewport;
  return { inner: innerWidth + 'x' + innerHeight, outer: outerWidth + 'x' + outerHeight,
    screen: screen.width + 'x' + screen.height, avail: screen.availHeight, vv: vv ? Math.round(vv.height) : null,
    bar: screen.height - innerHeight, scrollY: Math.round(scrollY), fs: !!document.fullscreenElement,
    ua: navigator.userActivation ? (navigator.userActivation.isActive ? 'active' : navigator.userActivation.hasBeenActive ? 'sticky' : 'none') : '?',
    vis: document.visibilityState, mode: ['fullscreen', 'standalone', 'minimal-ui', 'browser'].find(m => matchMedia('(display-mode: ' + m + ')').matches) };
}
function send(ev, extra) {
  const m = Object.assign({ ev, n: seq++, ms: Math.round(performance.now() - t0) }, geo(), extra || {});
  document.getElementById('s').textContent = JSON.stringify(m, null, 1);
  fetch('/log/' + name, { method: 'POST', body: JSON.stringify(m), keepalive: true }).catch(() => {});
}
function tryFs(why) {
  if (document.fullscreenElement) return;
  let p;
  try { p = document.documentElement.requestFullscreen({ navigationUI: 'hide' }); }
  catch (e) { send('fs-throw', { why, e: String(e) }); return; }
  p.then(() => { if (!got) got = why; send('fs-ok', { why }); }, e => send('fs-err', { why, e: String(e) }));
}
send('script', { api: {
  getScreenDetails: typeof window.getScreenDetails, keyboardLock: !!(navigator.keyboard && navigator.keyboard.lock),
  closeWatcher: typeof window.CloseWatcher, orientationLock: !!(screen.orientation && screen.orientation.lock),
  fullscreenPolicy: document.featurePolicy ? document.featurePolicy.allowsFeature('fullscreen') : '?',
  fullscreenEnabled: document.fullscreenEnabled } });
if (flags.has('wm') && window.getScreenDetails) getScreenDetails().then(() => { send('screen-details'); tryFs('after-getScreenDetails'); }, e => send('screen-details-err', { e: String(e) }));
if (flags.has('kl') && navigator.keyboard && navigator.keyboard.lock) navigator.keyboard.lock().then(() => send('keyboard-lock-ok'), e => send('keyboard-lock-err', { e: String(e) }));
if (flags.has('load')) {
  tryFs('script');
  document.addEventListener('DOMContentLoaded', () => tryFs('domcontentloaded'));
  addEventListener('load', () => tryFs('load'));
  requestAnimationFrame(() => tryFs('raf'));
  for (const ms of [0, 500, 2000]) setTimeout(() => tryFs('timeout' + ms), ms);
  addEventListener('pageshow', () => tryFs('pageshow'));
  document.addEventListener('visibilitychange', () => tryFs('visibilitychange'));
  addEventListener('focus', () => tryFs('focus'));
}
if (flags.has('scroll')) {
  addEventListener('load', () => {
    setTimeout(() => { scrollTo(0, 1); send('scrolled-1'); }, 300);
    setTimeout(() => { scrollTo({ top: 400, behavior: 'smooth' }); send('scrolled-400'); }, 1200);
    setTimeout(() => { document.getElementById('end').scrollIntoView(); send('scrolled-end'); }, 2500);
    setTimeout(() => { location.hash = '#end'; send('fragment'); }, 3500);
  });
}
if (flags.has('events')) {
  for (const ev of ['touchstart', 'pointerdown', 'mousedown', 'touchend', 'pointerup', 'mouseup', 'click', 'keydown', 'resize', 'focus', 'blur'])
    addEventListener(ev, e => { send('event', { type: ev }); tryFs(ev); }, { capture: true, passive: true });
}
addEventListener('resize', () => send('resize'));
document.addEventListener('fullscreenchange', () => send('fullscreenchange'));
addEventListener('load', () => send('load'));
setTimeout(() => send('t3s'), 3000);
setTimeout(() => send('t6s'), 6000);
</script></body></html>"""

MANIFEST = json.dumps({"name": "fs probe", "start_url": ".", "display": "fullscreen",
                       "display_override": ["fullscreen", "standalone"], "background_color": "#0f1720",
                       "theme_color": "#0f1720", "icons": []})


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def reply(self, code, ctype, body):
        b = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(b)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        path, _, query = self.path.partition("?")
        flags = set(f.split("=")[0] for f in query.split("&") if f)
        if path.startswith("/p/"):
            name = path[3:].strip("/").split("#")[0]
            page = (PAGE.replace("NAME", name)
                    .replace("TALL", "tall" if flags & {"scroll", "tall"} else "")
                    .replace("VIEWPORT", ",viewport-fit=cover" if "theme" in flags else "")
                    .replace("THEME", '<meta name=theme-color content="#0f1720">' if "theme" in flags else "")
                    .replace("MANIFEST", '<link rel=manifest href="/manifest.json">' if "manifest" in flags else ""))
            log("GET", self.path, self.headers.get("User-Agent", "")[:60])
            self.reply(200, "text/html", page)
        elif path == "/manifest.json":
            self.reply(200, "application/manifest+json", MANIFEST)
        elif path.startswith("/state/"):
            self.reply(200, "application/json", json.dumps(STATE.get(path[7:], {})))
        else:
            self.reply(404, "text/plain", "no")

    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("Content-Length", 0))).decode()
        name = self.path.split("/")[-1]
        try:
            m = json.loads(body)
        except ValueError:
            m = {"raw": body}
        st = STATE.setdefault(name, {})
        st["last"] = m
        if m.get("ev") == "fs-ok":
            st.setdefault("fs_ok", m.get("why"))
        st.setdefault("events", []).append(m.get("ev") + (":" + m["why"] if "why" in m else "") + (":" + m["type"] if "type" in m else ""))
        if "api" in m:
            st["api"] = m["api"]
        log("EV", name, json.dumps(m))
        self.reply(204, "text/plain", "")


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
