#!/usr/bin/env node
// Stuck modifiers in the web client: Shift (and Caps Lock's state) must not outlive the page's
// own knowledge of them. Drives crates/lindroids-server/data/web/lindroids.js in headless
// Chromium, with the link (WebSocket) replaced by a recorder that answers like the server.
//
//   node tests/stuck-input.mjs [--js <lindroids.js>] [--evidence <file.json>]
//
// --js: the lindroids.js under test (default: the repo's copy). Also LINDROIDS_JS in the environment.
// Environment:
//   LINDROIDS_PLAYWRIGHT_CORE    directory of a playwright-core package (or its index.js), if
//                                the search below does not find one
//
// playwright-core is not a dependency of this repo: it is found from LINDROIDS_PLAYWRIGHT_CORE,
// from the nearest node_modules, or from a local Playwright install on this machine. Chromium is
// the one Playwright cached (~/.cache/ms-playwright).
//
// Caps Lock: the headless browser's lock state cannot be toggled reliably, so KeyboardEvent and
// MouseEvent getModifierState('CapsLock') are answered from window.__caps. The keys themselves
// are real (Playwright's and CDP's), and so are the pointer events (CDP's Input domain).
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const webDir = resolve(here, '../crates/lindroids-server/data/web');
const argOf = (flag) => { const i = process.argv.indexOf(flag); return i > 0 ? process.argv[i + 1] : null; };
const jsPath = resolve(argOf('--js') || process.env.LINDROIDS_JS || join(webDir, 'lindroids.js'));
const evidenceArg = argOf('--evidence');
const evidencePath = evidenceArg ? resolve(evidenceArg) : null;

const require = createRequire(import.meta.url);
function loadPlaywright() {
  const candidates = [
    process.env.LINDROIDS_PLAYWRIGHT_CORE,
    'playwright-core',
    'playwright',
    '/var/home/ratnode2/Projects/ish-scroll-test/node_modules/playwright-core',
  ].filter(Boolean);
  for (const c of candidates) {
    try { return require(c).chromium; } catch { /* next */ }
  }
  throw new Error('playwright-core not found: set LINDROIDS_PLAYWRIGHT_CORE to its directory');
}
const chromium = loadPlaywright();

// ------------------------------------------------------------------ the page's server

// lindroids.js gets the placeholders web.rs fills in; index.html and draw.js are served as they are.
function serve() {
  const js = readFileSync(jsPath, 'utf8').replace('@BUILD@', 'test').replace('@PICKER_TOKEN@', 'test-picker');
  const files = {
    '/': ['text/html; charset=utf-8', readFileSync(join(webDir, 'index.html'))],
    '/index.html': ['text/html; charset=utf-8', readFileSync(join(webDir, 'index.html'))],
    '/lindroids.js': ['text/javascript; charset=utf-8', js],
    '/draw.js': ['text/javascript; charset=utf-8', readFileSync(join(webDir, 'draw.js'))],
    '/icon.svg': ['image/svg+xml', readFileSync(join(webDir, 'icon.svg'))],
  };
  return new Promise((resolveServer) => {
    const server = createServer((req, res) => {
      const f = files[new URL(req.url, 'http://x').pathname];
      if (!f) { res.writeHead(404); return res.end('not found'); }
      res.writeHead(200, { 'content-type': f[0], 'cache-control': 'no-store' });
      res.end(f[1]);
    });
    server.listen(0, '127.0.0.1', () => resolveServer(server));
  });
}

// Runs before the page's own scripts. The WebSocket records what the page sends (one object per
// line, in order) and lets the test play the server; the hidden state and Caps Lock are controlled.
const INIT = `(() => {
  window.__sent = [];
  window.__sockets = [];
  window.__hidden = false;
  window.__caps = false;
  class FakeSocket {
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.index = window.__sockets.length;
      window.__sockets.push(this);
      setTimeout(() => { this.readyState = 1; if (this.onopen) this.onopen({}); }, 0);
    }
    send(data) {
      if (typeof data === 'string') {
        for (const line of data.split('\\n')) if (line) window.__sent.push({ socket: this.index, ...JSON.parse(line) });
      } else {
        window.__sent.push({ socket: this.index, t: '<binary>' });
      }
    }
    close(code) {
      if (this.readyState === 3) return;
      this.readyState = 3;
      if (this.onclose) this.onclose({ code: code || 1000 });
    }
  }
  FakeSocket.CONNECTING = 0; FakeSocket.OPEN = 1; FakeSocket.CLOSED = 3;
  window.WebSocket = FakeSocket;
  // The server's side: one message, as the page's onmessage gets it (no payload).
  window.__deliver = (msg, index) => {
    const s = window.__sockets[index === undefined ? window.__sockets.length - 1 : index];
    s.onmessage({ data: new TextEncoder().encode(JSON.stringify(msg)).buffer });
  };
  Object.defineProperty(Document.prototype, 'hidden', { configurable: true, get() { return window.__hidden; } });
  Object.defineProperty(Document.prototype, 'visibilityState', { configurable: true, get() { return window.__hidden ? 'hidden' : 'visible'; } });
  for (const proto of [KeyboardEvent.prototype, MouseEvent.prototype]) {
    const orig = proto.getModifierState;
    proto.getModifierState = function (k) { return k === 'CapsLock' ? window.__caps : orig.call(this, k); };
  }
})();`;

// ------------------------------------------------------------------ helpers

const results = [];
function record(name, pass, detail, messages) {
  results.push({ name, pass, detail, messages });
  console.log(`${pass ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
}

async function openPage(browser, server) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 720 } });
  await context.addInitScript(INIT);
  const page = await context.newPage();
  page.on('pageerror', (e) => console.log(`  [page error] ${e.message}`));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  // The page says hello; the server answers, then opens a window and gives it the focus.
  await page.waitForFunction(() => window.__sent.some((m) => m.t === 'hello'));
  await page.evaluate(() => window.__deliver({ t: 'hello' }));
  await page.evaluate(() => window.__deliver({ t: 'window_new', win: 1, title: 'Test window', app_id: 'test' }));
  await page.waitForFunction(() => document.activeElement && document.activeElement.tagName === 'CANVAS');
  await settle(page);
  return { context, page };
}

// Messages are batched to the microtask; a short wait lets them land.
const settle = (page) => page.waitForTimeout(60);
const mark = (page) => page.evaluate(() => window.__sent.length);
const since = (page, from) => page.evaluate((f) => window.__sent.slice(f), from);
const isKey = (m, code, down) => m.t === 'key' && m.code === code && (down === undefined || m.down === down);
const keyEvents = (msgs) => msgs.filter((m) => m.t === 'key').map((m) => `${m.code}${m.down ? '↓' : '↑'}`).join(' ');

async function reconnect(page) {
  const before = await page.evaluate(() => window.__sockets.length);
  await page.evaluate(() => window.__sockets[window.__sockets.length - 1].close(1006));
  // The page waits about two seconds, then links again.
  await page.waitForFunction((n) => window.__sockets.length > n, before, { timeout: 10000 });
  const index = before;
  await page.waitForFunction((i) => window.__sent.some((m) => m.socket === i && m.t === 'hello'), index);
  await page.evaluate((i) => window.__deliver({ t: 'hello' }, i), index);
  await page.evaluate((i) => window.__deliver({ t: 'window_new', win: 1, title: 'Test window', app_id: 'test' }, i), index);
  await page.waitForFunction(() => document.activeElement && document.activeElement.tagName === 'CANVAS');
  await settle(page);
  return index;
}

// ------------------------------------------------------------------ cases

async function caseBlur(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await settle(page);
  const msgs = await since(page, from);
  const up = msgs.some((m) => isKey(m, 59, false));
  record('1 window blur releases Shift', up, up ? `sent ${keyEvents(msgs)}` : `no key 59 up (sent: ${keyEvents(msgs) || 'nothing'})`, msgs);
  await page.keyboard.up('ShiftLeft');
  await context.close();
}

async function caseHidden(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  await page.evaluate(() => { window.__hidden = true; document.dispatchEvent(new Event('visibilitychange')); });
  await settle(page);
  const msgs = await since(page, from);
  const up = msgs.some((m) => isKey(m, 59, false));
  record('2 page hidden releases Shift', up, up ? `sent ${keyEvents(msgs)}` : `no key 59 up (sent: ${keyEvents(msgs) || 'nothing'})`, msgs);
  await page.evaluate(() => { window.__hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
  await page.keyboard.up('ShiftLeft');
  await context.close();
}

async function caseReconnect(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  const index = await reconnect(page);
  // Shift is still down on the keyboard, but the link is new: the old page state is gone, so
  // nothing about Shift may be sent on the new link, and a later blur must not release it.
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await settle(page);
  const all = await since(page, from);
  const onNew = all.filter((m) => m.socket === index);
  const shiftOnNew = onNew.filter((m) => m.t === 'key' && m.code === 59);
  const pass = shiftOnNew.length === 0;
  record('3 link lost and back: Shift state forgotten, not re-sent or released later', pass,
    pass ? 'nothing about key 59 on the new link' : `new link sent ${keyEvents(shiftOnNew)}`, all);
  await page.keyboard.up('ShiftLeft');
  await context.close();
}

async function caseLostKeyUp(browser, server) {
  const { context, page } = await openPage(browser, server);
  const cdp = await context.newCDPSession(page);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  // A pointer press with no Shift: the key-up for Shift was lost (it went elsewhere).
  await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: 640, y: 360, button: 'left', buttons: 1, clickCount: 1, modifiers: 0 });
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: 640, y: 360, button: 'left', buttons: 0, clickCount: 1, modifiers: 0 });
  await settle(page);
  const msgs = await since(page, from);
  const upAt = msgs.findIndex((m) => isKey(m, 59, false));
  const pressAt = msgs.findIndex((m) => m.t === 'pointer' && m.action === 'press');
  const pass = upAt >= 0 && pressAt >= 0 && upAt < pressAt;
  record('4 pointer without Shift releases a lost Shift before the press', pass,
    pass ? `sent ${keyEvents(msgs)} then pointer press` : `key 59 up at ${upAt}, press at ${pressAt} (sent: ${keyEvents(msgs) || 'nothing'})`, msgs);
  await page.keyboard.up('ShiftLeft');
  await context.close();
}

async function caseCaps(browser, server) {
  const { context, page } = await openPage(browser, server);
  const caps = (msgs) => msgs.filter((m) => m.t === 'caps_lock').map((m) => m.on);

  // Caps Lock on, then off, by its key.
  let from = await mark(page);
  await page.evaluate(() => { window.__caps = true; });
  await page.keyboard.press('CapsLock');
  await settle(page);
  let msgs = await since(page, from);
  let got = caps(msgs);
  record('5a Caps Lock on sends caps_lock {on:true}', got.length === 1 && got[0] === true, `caps_lock sent: ${JSON.stringify(got)}`, msgs);

  from = await mark(page);
  await page.evaluate(() => { window.__caps = false; });
  await page.keyboard.press('CapsLock');
  await settle(page);
  msgs = await since(page, from);
  got = caps(msgs);
  record('5a Caps Lock off sends caps_lock {on:false}', got.length === 1 && got[0] === false, `caps_lock sent: ${JSON.stringify(got)}`, msgs);

  // After a blur, the next key sends the page's real Caps state again (the relay's copy is no
  // longer trusted), even though it did not change.
  await page.evaluate(() => { window.__caps = true; });
  await page.keyboard.press('CapsLock');
  await settle(page);
  from = await mark(page);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await settle(page);
  await page.keyboard.press('KeyB');
  await settle(page);
  msgs = await since(page, from);
  got = caps(msgs);
  record('5b after blur, the next key re-sends caps_lock {on:true}', got.length >= 1 && got[0] === true, `caps_lock sent: ${JSON.stringify(got)}`, msgs);
  await page.evaluate(() => { window.__caps = false; });
  await page.keyboard.press('CapsLock');
  await settle(page);

  // After a reconnect, the first event sends the page's state, here off.
  from = await mark(page);
  await reconnect(page);
  await page.keyboard.press('KeyD');
  await settle(page);
  msgs = await since(page, from);
  got = caps(msgs);
  record('5c after reconnect, the first key sends caps_lock {on:false}', got.length >= 1 && got[0] === false, `caps_lock sent: ${JSON.stringify(got)}`, msgs);

  // Typing a letter with Caps Lock off: a plain 'a', no Shift made up around it.
  from = await mark(page);
  await page.keyboard.press('KeyA');
  await settle(page);
  msgs = await since(page, from);
  const letter = keyEvents(msgs);
  const shiftSent = msgs.some((m) => m.t === 'key' && m.code === 59);
  const pass = letter === '29↓ 29↑' && !shiftSent;
  record('5d typing "a" with Caps Lock off sends the plain key, no synthesized Shift', pass, `sent ${letter || 'nothing'}`, msgs);
  await context.close();
}

// Events the test makes itself, as the browser would make them: the flags, AltGraph state and
// pointer type say what a soft keyboard, an AltGr key or a finger would say, on the focused element.
async function synthKey(page, type, init) {
  await page.evaluate(([t, i]) => {
    (document.activeElement || document.body).dispatchEvent(new KeyboardEvent(t, { bubbles: true, cancelable: true, ...i }));
  }, [type, init]);
}
async function synthPointer(page, type, init) {
  await page.evaluate(([t, i]) => {
    (document.activeElement || document.body).dispatchEvent(new PointerEvent(t, { bubbles: true, cancelable: true, ...i }));
  }, [type, init]);
}

// A letter typed with only the Shift flag (no Shift key): a Shift is borrowed for it, and one
// Shift serves both letters held together, going up with the last.
async function caseFlagShift(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await synthKey(page, 'keydown', { code: 'KeyA', key: 'A', shiftKey: true });
  await synthKey(page, 'keydown', { code: 'KeyB', key: 'B', shiftKey: true });
  await synthKey(page, 'keyup', { code: 'KeyA', key: 'A', shiftKey: false });
  await settle(page);
  const mid = await since(page, from);
  await synthKey(page, 'keyup', { code: 'KeyB', key: 'B', shiftKey: false });
  await settle(page);
  const msgs = await since(page, from);
  const got = keyEvents(msgs);
  const pass = got === '59↓ 29↓ 30↓ 29↑ 30↑ 59↑' && keyEvents(mid) === '59↓ 29↓ 30↓ 29↑';
  record('6 flag-only Shift: 59 down, then the letter; no 59 up before the letter ends', pass, `sent ${got || 'nothing'}`, msgs);
  await context.close();
}

// AltGr is held, then a letter: AltGr is Alt down (code 58), whatever the Alt flag says.
async function caseAltGr(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('AltRight');
  await settle(page);
  await synthKey(page, 'keydown', { code: 'KeyE', key: 'e', modifierAltGraph: true });
  await synthKey(page, 'keyup', { code: 'KeyE', key: 'e', modifierAltGraph: true });
  await settle(page);
  const msgs = await since(page, from);
  const pass = !msgs.some((m) => isKey(m, 58, false)) && keyEvents(msgs) === '58↓ 33↓ 33↑';
  record('7 AltGr held, then a letter: no key 58 up', pass, `sent ${keyEvents(msgs) || 'nothing'}`, msgs);
  await page.keyboard.up('AltRight');
  await context.close();
}

// A finger's pointer event says Shift is up (touch flags say nothing about the keyboard): Shift stays.
async function caseTouchFlags(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  await synthPointer(page, 'pointerdown', { pointerType: 'touch', isPrimary: true, pointerId: 7, button: 0, buttons: 1, shiftKey: false });
  await settle(page);
  const during = await since(page, from);
  const kept = !during.some((m) => isKey(m, 59, false));
  await page.keyboard.up('ShiftLeft');
  await settle(page);
  const msgs = await since(page, from);
  const released = msgs.some((m) => isKey(m, 59, false));
  const pass = kept && released;
  record('8 touch pointerdown with Shift flag up: no key 59 up', pass,
    !kept ? `touch released Shift (sent: ${keyEvents(during)})` : released ? 'Shift kept on touch, released by its own key-up' : 'Shift kept, but its own key-up sent nothing', msgs);
  await context.close();
}

// A window switch with Shift held: the Shift goes up in the window it was pressed in, first.
async function caseWindowSwitch(browser, server) {
  const { context, page } = await openPage(browser, server);
  await page.evaluate(() => window.__deliver({ t: 'window_new', win: 2, title: 'Second window', app_id: 'test' }));
  await settle(page);
  await page.evaluate(() => window.__deliver({ t: 'window_activate', win: 1 }));
  await settle(page);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  await page.evaluate(() => window.__deliver({ t: 'window_activate', win: 2 }));
  await settle(page);
  const msgs = await since(page, from);
  const upAt = msgs.findIndex((m) => isKey(m, 59, false));
  const focusAt = msgs.findIndex((m) => m.t === 'focus' && m.win === 1 && m.focused === false);
  const pass = upAt >= 0 && msgs[upAt].win === 1 && (focusAt < 0 || upAt < focusAt);
  record('9 window switch with Shift held: key 59 up goes to the old window first', pass,
    pass ? `sent ${keyEvents(msgs)} to win ${msgs[upAt].win}` : `up at ${upAt} (sent: ${keyEvents(msgs) || 'nothing'})`, msgs);
  await page.keyboard.up('ShiftLeft');
  await context.close();
}

// Two Shifts: the event for one of them says Shift is up, but the other is still down.
async function caseTwoShifts(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await page.keyboard.down('ShiftRight');
  await settle(page);
  await synthKey(page, 'keyup', { code: 'ShiftLeft', key: 'Shift', shiftKey: false });
  await settle(page);
  const mid = await since(page, from);
  const pass = mid.some((m) => isKey(m, 59, false)) && !mid.some((m) => isKey(m, 60, false));
  record('10 two Shifts, one up with no Shift flag: the other is not released', pass,
    `sent ${keyEvents(mid) || 'nothing'}`, mid);
  await page.keyboard.up('ShiftRight');
  await page.keyboard.up('ShiftLeft');
  await context.close();
}

// Meta's flag is not reconciled: a pointer event without it does not release Meta.
async function caseMetaNotReconciled(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('MetaLeft');
  await settle(page);
  await synthPointer(page, 'pointerdown', { pointerType: 'mouse', isPrimary: true, pointerId: 1, button: 0, buttons: 1, metaKey: false, shiftKey: false });
  await settle(page);
  const during = await since(page, from);
  const pass = !during.some((m) => isKey(m, 117, false));
  record('11 Meta is not reconciled: a pointer without the Meta flag does not release it', pass,
    pass ? 'no key 117 up while pointing' : `sent ${keyEvents(during)}`, during);
  await page.keyboard.up('MetaLeft');
  await settle(page);
  await context.close();
}

// ------------------------------------------------------------------ main

const server = await serve();
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
try {
  console.log(`testing ${jsPath}`);
  await caseBlur(browser, server);
  await caseHidden(browser, server);
  await caseReconnect(browser, server);
  await caseLostKeyUp(browser, server);
  await caseCaps(browser, server);
  await caseFlagShift(browser, server);
  await caseAltGr(browser, server);
  await caseTouchFlags(browser, server);
  await caseWindowSwitch(browser, server);
  await caseTwoShifts(browser, server);
  await caseMetaNotReconciled(browser, server);
} finally {
  await browser.close();
  server.close();
}

const failed = results.filter((r) => !r.pass).length;
console.log(`${results.length - failed}/${results.length} passed`);
if (evidencePath) {
  mkdirSync(dirname(evidencePath), { recursive: true });
  writeFileSync(evidencePath, JSON.stringify({ file: jsPath, when: new Date().toISOString(), passed: results.length - failed, failed, cases: results }, null, 2) + '\n');
  console.log(`evidence: ${evidencePath}`);
}
process.exit(failed ? 1 : 0);
