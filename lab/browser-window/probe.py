#!/usr/bin/env python3
"""Browser display experiments: standalone windows, intents, close detection, origins.

Serves /app/<name>/ (an installable page per name), /ws (a WebSocket that logs everything the
page reports), /cmd/<cmd>/<name> (send a command to that page). Logs to ~/exp.log.
"""
import asyncio, base64, hashlib, json, os, struct, sys, time, zlib

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 48100
LOG = os.path.expanduser("~/exp.log")
T0 = time.time()
socks = {}  # name -> writer


def log(*a):
    line = "%8.2f %s" % (time.time() - T0, " ".join(str(x) for x in a))
    print(line, flush=True)
    with open(LOG, "a") as f:
        f.write(line + "\n")


def png(size, rgb):
    raw = b"".join(b"\0" + bytes(rgb) * size for _ in range(size))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


PAGE = r"""<!doctype html><html><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<link rel=manifest href="manifest.json"><link rel=icon href="icon-192.png">
<title>NAME</title></head><body style="background:#123;color:#fff;font:20px sans-serif">
<h1>NAME</h1><pre id=o></pre><button id=inst hidden>Install</button>
<script>
const name = "NAME", o = document.getElementById('o');
const out = s => o.textContent += s + "\n";
let ws, n = 0;
function send(ev, extra) {
  const m = JSON.stringify(Object.assign({ev, name, t: Date.now(), vis: document.visibilityState}, extra || {}));
  if (ws && ws.readyState === 1) ws.send(m); else navigator.sendBeacon('/beacon', m);
}
function connect() {
  ws = new WebSocket((location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/ws?name=' + name);
  ws.onopen = () => send('open', {
    href: location.href, secure: isSecureContext, subtle: !!(crypto && crypto.subtle),
    standalone: matchMedia('(display-mode: standalone)').matches,
    fullscreen: matchMedia('(display-mode: fullscreen)').matches,
    minimal: matchMedia('(display-mode: minimal-ui)').matches,
    browser: matchMedia('(display-mode: browser)').matches,
    hist: history.length, ref: document.referrer, ua: navigator.userAgent, n: n++ });
  ws.onmessage = e => {
    out('cmd ' + e.data);
    if (e.data === 'close') { send('closing'); window.close(); setTimeout(() => send('still-open'), 1000); }
    if (e.data === 'ping') send('pong');
  };
  ws.onclose = e => { out('ws closed ' + e.code); setTimeout(connect, 1000); };
}
connect();
for (const ev of ['pagehide', 'pageshow', 'visibilitychange', 'freeze', 'resume', 'beforeunload', 'unload'])
  addEventListener(ev, e => send(ev, {persisted: e.persisted}), {capture: true});
document.addEventListener('visibilitychange', () => send('vis'));
document.addEventListener('freeze', () => send('freeze-doc'));
document.addEventListener('resume', () => send('resume-doc'));
setInterval(() => send('beat'), 5000);
addEventListener('click', () => {
  const el = document.documentElement;
  if (!document.fullscreenElement && el.requestFullscreen)
    el.requestFullscreen({navigationUI: 'hide'}).then(() => send('fs-ok'), e => send('fs-err', {e: String(e)}));
});
let prompt;
addEventListener('beforeinstallprompt', e => { e.preventDefault(); prompt = e; inst.hidden = false; send('installable'); });
addEventListener('appinstalled', () => send('installed'));
inst.onclick = async () => { prompt.prompt(); const r = await prompt.userChoice; send('choice', {r: r.outcome}); };
</script></body></html>"""


async def ws_session(name, reader, writer):
    socks[name] = writer
    log("WS-OPEN", name)
    try:
        while True:
            h = await reader.readexactly(2)
            op, ln = h[0] & 0x0F, h[1] & 0x7F
            if ln == 126: ln = struct.unpack(">H", await reader.readexactly(2))[0]
            elif ln == 127: ln = struct.unpack(">Q", await reader.readexactly(8))[0]
            mask = await reader.readexactly(4) if h[1] & 0x80 else b"\0\0\0\0"
            data = bytes(b ^ mask[i % 4] for i, b in enumerate(await reader.readexactly(ln)))
            if op == 8:
                code = struct.unpack(">H", data[:2])[0] if len(data) >= 2 else None
                log("WS-CLOSE-FRAME", name, code, data[2:].decode(errors="replace"))
                writer.write(b"\x88\x00"); break
            if op == 1:
                m = json.loads(data)
                if m["ev"] != "beat" or "-v" in sys.argv:
                    log("EV", name, json.dumps({k: v for k, v in m.items() if k not in ("name", "t")}))
                elif m["ev"] == "beat":
                    log("beat", name, m["vis"])
    except (asyncio.IncompleteReadError, ConnectionError) as e:
        log("WS-EOF", name, type(e).__name__)
    finally:
        if socks.get(name) is writer: del socks[name]
        writer.close()


async def ws_send(writer, text):
    d = text.encode()
    writer.write(bytes([0x81, len(d)]) + d)
    await writer.drain()


async def handle(reader, writer):
    try:
        req = await reader.readuntil(b"\r\n\r\n")
    except Exception:
        writer.close(); return
    lines = req.decode().split("\r\n")
    method, path, _ = lines[0].split(" ", 2)
    hdr = {l.split(":", 1)[0].lower(): l.split(":", 1)[1].strip() for l in lines[1:] if ":" in l}
    body = b""
    if "content-length" in hdr:
        body = await reader.readexactly(int(hdr["content-length"]))

    def reply(status, ctype, data):
        writer.write(("HTTP/1.1 %s\r\nContent-Type: %s\r\nContent-Length: %d\r\nCache-Control: no-store\r\n"
                      "Connection: close\r\n\r\n" % (status, ctype, len(data))).encode() + data)

    if path.startswith("/ws"):
        name = path.split("name=", 1)[1]
        log("WS-UPGRADE", name, "host=" + hdr.get("host", ""), "origin=" + hdr.get("origin", ""))
        acc = base64.b64encode(hashlib.sha1((hdr["sec-websocket-key"] + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        writer.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                      "Sec-WebSocket-Accept: %s\r\n\r\n" % acc).encode())
        await ws_session(name, reader, writer); return
    if path == "/beacon":
        log("BEACON", body.decode(errors="replace")); reply("204 No Content", "text/plain", b"")
    elif path.startswith("/cmd/"):
        _, _, cmd, name = path.split("/", 3)
        w = socks.get(name)
        if w: await ws_send(w, cmd); reply("200 OK", "text/plain", b"sent\n")
        else: reply("404 Not Found", "text/plain", b"no such page\n")
    elif path.startswith("/app/"):
        parts = path.split("/")
        name, rest = parts[2], "/".join(parts[3:]).split("?")[0].split("#")[0]
        if rest == "manifest.json":
            m = {"name": "Exp " + name, "short_name": name, "start_url": "/app/%s/" % name,
                 "scope": "/app/%s/" % name, "display": "standalone", "background_color": "#112233",
                 "theme_color": "#112233", "id": "/app/%s/" % name,
                 "icons": [{"src": "icon-192.png", "sizes": "192x192", "type": "image/png"},
                           {"src": "icon-512.png", "sizes": "512x512", "type": "image/png"}]}
            reply("200 OK", "application/manifest+json", json.dumps(m).encode())
        elif rest.startswith("icon-"):
            size = int(rest[5:8])
            col = (zlib.crc32(name.encode()) & 0xFFFFFF).to_bytes(3, "big")
            reply("200 OK", "image/png", png(size, col))
        else:
            log("GET", path, "host=" + hdr.get("host", ""))
            reply("200 OK", "text/html", PAGE.replace("NAME", name).encode())
    else:
        reply("404 Not Found", "text/plain", b"")
    await writer.drain()
    writer.close()


async def main():
    srv = await asyncio.start_server(handle, "127.0.0.1", PORT)
    log("listening", PORT)
    async with srv:
        await srv.serve_forever()

asyncio.run(main())
