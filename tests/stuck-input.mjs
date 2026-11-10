#!/usr/bin/env node
// Stuck input in the web client: keys, modifiers, mouse buttons, Caps/Num Lock and window sizes
// must not outlive the page's own knowledge of them. Drives
// crates/lindroids-server/data/web/lindroids.js in headless Chromium, with the link (WebSocket)
// replaced by a recorder that answers like the server.
//
//   node tests/stuck-input.mjs [--js <lindroids.js>] [--evidence <file.json>]
//   node tests/stuck-input.mjs --stress [--seed N] [--iterations N] [--storms N]
//
// --js: the lindroids.js under test (default: the repo's copy). Also LINDROIDS_JS in the environment.
// --stress: seeded random storms instead of the cases (see `stress` below); --seed picks the first
// storm's seed (default 1), --storms how many storms (default 5, seeds N, N+1, ...), --iterations
// the input events in each (default 300). A failure prints the seed that reproduces it.
// Environment:
//   LINDROIDS_PLAYWRIGHT_CORE    directory of a playwright-core package (or its index.js), if
//                                the search below does not find one
//
// playwright-core is not a dependency of this repo: it is found from LINDROIDS_PLAYWRIGHT_CORE,
// from the nearest node_modules, or from a local Playwright install on this machine. Chromium is
// the one Playwright cached (~/.cache/ms-playwright).
//
// Caps Lock and Num Lock: the headless browser's lock state cannot be toggled reliably, so
// KeyboardEvent and MouseEvent getModifierState('CapsLock'/'NumLock') are answered from
// window.__caps and window.__num. The keys themselves
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
  window.__num = false;
  class FakeSocket {
    constructor(url) {
      this.url = url;
      this.readyState = 0;
      this.bufferedAmount = 0;
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
    proto.getModifierState = function (k) {
      return k === 'CapsLock' ? window.__caps : k === 'NumLock' ? window.__num : orig.call(this, k);
    };
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

// ------------------------------------------------------------------ the server's side

// The server's seat as seat.rs keeps it, fed the page's messages in order: the keys and buttons
// held on the Linux side, the focus, the locks, the last size. The server lets go of everything
// when the focused window loses the focus, its page hides or its link drops (a new socket) (the
// buttons with the window the pointer is in, focused or not), and only a key going down takes the
// focus. A key going down while down is a repeat (the server sends it as `repeated`, or nothing to
// programs that repeat on their own), never for a modifier; a page that presses a button already
// down, or repeats a modifier, makes a mistake (`errors`).
// Android key codes of the modifiers and locks: Alt, Shift, Ctrl, Meta (left and right), Caps
// Lock, Num Lock.
const ANDROID_MODIFIERS = new Set([57, 58, 59, 60, 113, 114, 117, 118, 115, 143]);
class SeatModel {
  constructor() {
    this.socket = -1; this.focus = null; this.keys = new Set(); this.buttons = new Set();
    this.locks = null; this.size = null; this.errors = [];
  }
  releaseAll() { this.keys.clear(); this.buttons.clear(); }
  feed(msgs) { for (const m of msgs) this.one(m); return this; }
  one(m) {
    if (m.socket !== this.socket) { this.socket = m.socket; this.releaseAll(); this.focus = null; }
    switch (m.t) {
      case 'focus':
        if (m.focused) { if (this.focus !== m.win) this.keys.clear(); this.focus = m.win; }
        else {
          // Buttons go with the window the pointer is in, focused or not; keys with the focus.
          if (this.focus === m.win || this.pointer === m.win) this.buttons.clear();
          if (this.focus === m.win) { this.focus = null; this.keys.clear(); }
        }
        break;
      case 'shown':
        if (!m.shown) { this.buttons.clear(); if (this.focus !== null) this.keys.clear(); }
        break;
      case 'key':
        if (this.focus !== m.win) {
          if (!m.down) break;
          this.keys.clear();
          this.focus = m.win;
        }
        if (m.down && this.keys.has(m.code) && ANDROID_MODIFIERS.has(m.code)) this.errors.push(`modifier ${m.code} repeated`);
        if (m.down) this.keys.add(m.code); else this.keys.delete(m.code);
        break;
      case 'pointer':
        if (!this.buttons.size) this.pointer = m.win;
        if (m.action === 'press' && this.buttons.has(m.button)) this.errors.push(`button ${m.button} pressed while down`);
        if (m.action === 'press') this.buttons.add(m.button);
        if (m.action === 'release') this.buttons.delete(m.button);
        break;
      case 'caps_lock': this.locks = [m.on, m.num]; break;
      case 'resize': this.size = { width: m.width, height: m.height, screen: m.screen }; break;
      default: break;
    }
  }
}
const modelOf = async (page) => new SeatModel().feed(await page.evaluate(() => window.__sent));

// ------------------------------------------------------------------ cases

// Blur: the window loses the focus on the Linux side (the server lets go of what it held there);
// the page forgets Shift, and its key-up later sends nothing.
async function caseBlur(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  await page.evaluate(() => window.dispatchEvent(new Event('blur')));
  await settle(page);
  await page.keyboard.up('ShiftLeft');
  await settle(page);
  const msgs = await since(page, from);
  const lost = msgs.findIndex((m) => m.t === 'focus' && m.win === 1 && m.focused === false);
  const model = await modelOf(page);
  const pass = lost >= 0 && !msgs.slice(lost).some((m) => m.t === 'key') && !model.keys.size;
  record('1 window blur: focus lost, Shift let go by the server, its key-up sends nothing', pass, `sent ${keyEvents(msgs)}; focus lost at ${lost}; server holds ${[...model.keys]}`, msgs);
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await settle(page);
  const back = await since(page, from);
  record('1b window focus again: focus true', back.some((m) => m.t === 'focus' && m.focused === true), '', back);
  await context.close();
}

// Hidden: one visibility handler, in order: focus lost, then shown false; no key-ups from the page.
async function caseHidden(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('ShiftLeft');
  await settle(page);
  await page.evaluate(() => { window.__hidden = true; document.dispatchEvent(new Event('visibilitychange')); });
  await settle(page);
  const msgs = (await since(page, from)).filter((m) => m.t !== 'log' && m.t !== 'caps_lock');
  const order = msgs.map((m) => m.t === 'key' ? `${m.code}${m.down ? '↓' : '↑'}` : m.t === 'focus' ? `focus:${m.focused}` : m.t === 'shown' ? `shown:${m.shown}` : m.t).join(' ');
  const model = await modelOf(page);
  record('2 page hidden: "59↓ focus:false shown:false", Shift let go by the server', order === '59↓ focus:false shown:false' && !model.keys.size, `sent ${order}`, msgs);
  const at = await mark(page);
  await page.evaluate(() => { window.__hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
  await page.waitForTimeout(500);
  const back = (await since(page, at)).filter((m) => m.t !== 'log');
  const backOrder = back.map((m) => m.t === 'focus' ? `focus:${m.focused}` : m.t === 'shown' ? `shown:${m.shown}` : m.t).join(' ');
  record('2b page shown: "focus:true shown:true", and no resize (the size did not change)', backOrder === 'focus:true shown:true', `sent ${backOrder}`, back);
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

// A letter typed with only the Shift flag (no Shift key): Shift goes down from the flag, before the
// letter, and up when a key event says it is up.
async function caseFlagShift(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await synthKey(page, 'keydown', { code: 'KeyA', key: 'A', shiftKey: true });
  await synthKey(page, 'keydown', { code: 'KeyB', key: 'B', shiftKey: true });
  await synthKey(page, 'keyup', { code: 'KeyA', key: 'A', shiftKey: false });
  await synthKey(page, 'keyup', { code: 'KeyB', key: 'B', shiftKey: false });
  await settle(page);
  const msgs = await since(page, from);
  const got = keyEvents(msgs);
  record('6 flag-only Shift: pressed from the flag before the letters, released when a key says it is up', got === '59↓ 29↓ 30↓ 59↑ 29↑ 30↑', `sent ${got || 'nothing'}`, msgs);
  // A capital letter with no Shift flag at all (and Caps Lock off): Shift around it.
  const at = await mark(page);
  await synthKey(page, 'keydown', { code: 'KeyC', key: 'C' });
  await synthKey(page, 'keyup', { code: 'KeyC', key: 'C' });
  await settle(page);
  const cap = keyEvents(await since(page, at));
  record('6b capital letter with no Shift flag: Shift around it', cap === '59↓ 31↓ 59↑ 31↑', `sent ${cap}`);
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

// A window switch with Shift held: the old window loses the focus first (the server lets go of the
// Shift there), and the Shift's key-up later goes nowhere.
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
  await page.keyboard.up('ShiftLeft');
  await settle(page);
  const msgs = await since(page, from);
  const lostAt = msgs.findIndex((m) => m.t === 'focus' && m.win === 1 && m.focused === false);
  const gainAt = msgs.findIndex((m) => m.t === 'focus' && m.win === 2 && m.focused === true);
  const model = await modelOf(page);
  const pass = lostAt >= 0 && lostAt < gainAt && keyEvents(msgs) === '59↓' && !model.keys.size;
  record('9 window switch with Shift held: win 1 loses the focus before win 2 gets it; nothing left held', pass,
    `sent ${keyEvents(msgs)}, focus lost at ${lostAt}, gained at ${gainAt}`, msgs);
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

// Meta's flag is not used from pointer events: a click without it does not release Meta. A key
// event without it does (Samsung keeps Meta's key-up for itself).
async function caseMeta(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('MetaLeft');
  await settle(page);
  await synthPointer(page, 'pointerdown', { pointerType: 'mouse', isPrimary: true, pointerId: 1, button: 0, buttons: 1, metaKey: false, shiftKey: false });
  await settle(page);
  const during = await since(page, from);
  record('11 Meta: a pointer without the Meta flag does not release it', !during.some((m) => isKey(m, 117, false)), `sent ${keyEvents(during)}`, during);
  const at = await mark(page);
  await synthKey(page, 'keydown', { code: 'KeyD', key: 'd', metaKey: false });
  await settle(page);
  const after = keyEvents(await since(page, at));
  record('11b Meta: a key without the Meta flag releases a Meta whose key-up was lost, first', after.startsWith('117↑ 32↓'), `sent ${after}`);
  await synthKey(page, 'keyup', { code: 'KeyD', key: 'd' });
  await page.keyboard.up('MetaLeft');
  await context.close();
}

// Key repeat: the browser's auto-repeat goes as the key down again, never a release and a press
// (a game takes that for a new key-down); modifiers do not repeat.
async function caseRepeat(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('KeyA');
  await page.keyboard.down('KeyA'); // Playwright: a repeat (KeyboardEvent.repeat)
  await page.keyboard.down('KeyA');
  await page.keyboard.up('KeyA');
  await page.keyboard.down('ShiftLeft');
  await page.keyboard.down('ShiftLeft');
  await page.keyboard.up('ShiftLeft');
  await settle(page);
  const msgs = await since(page, from);
  const got = keyEvents(msgs);
  const model = await modelOf(page);
  record('12 auto-repeat: down again, no release in between; Shift does not repeat', got === '29↓ 29↓ 29↓ 29↑ 59↓ 59↑' && !model.errors.length,
    `sent ${got}${model.errors.length ? `; ${model.errors}` : ''}`, msgs);
  await context.close();
}

// A key-up in a composition (keyCode 229, isComposing) still lets go of the key it names.
async function caseImeKeyUp(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await page.keyboard.down('KeyA');
  await settle(page);
  await page.evaluate(() => {
    const e = new KeyboardEvent('keyup', { bubbles: true, cancelable: true, code: 'KeyA', key: 'Process', isComposing: true });
    Object.defineProperty(e, 'keyCode', { get: () => 229 });
    document.activeElement.dispatchEvent(e);
  });
  // And a key going down in a composition is the IME's: not sent.
  await page.evaluate(() => {
    const e = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, code: 'KeyB', key: 'Process', isComposing: true });
    Object.defineProperty(e, 'keyCode', { get: () => 229 });
    document.activeElement.dispatchEvent(e);
  });
  await settle(page);
  const got = keyEvents(await since(page, from));
  record('13 key-up with keyCode 229 releases the held key; a composing key-down is not sent', got === '29↓ 29↑', `sent ${got}`);
  await page.keyboard.up('KeyA');
  await context.close();
}

// A modifier the event says is down, with neither key held: pressed before the key.
async function caseModifierPressedFromFlag(browser, server) {
  const { context, page } = await openPage(browser, server);
  const from = await mark(page);
  await synthKey(page, 'keydown', { code: 'KeyC', key: 'c', ctrlKey: true });
  await synthKey(page, 'keyup', { code: 'KeyC', key: 'c', ctrlKey: true });
  await synthKey(page, 'keydown', { code: 'KeyX', key: 'x' });
  await synthKey(page, 'keyup', { code: 'KeyX', key: 'x' });
  await settle(page);
  const got = keyEvents(await since(page, from));
  record('14 Ctrl flag with no Ctrl held: 113 down before the key, up at the next key without it', got === '113↓ 31↓ 31↑ 113↑ 52↓ 52↑', `sent ${got}`);
  await context.close();
}

// Locks: from key events only (with their code), Num Lock with them; a touch says nothing.
async function caseLocks(browser, server) {
  const { context, page } = await openPage(browser, server);
  const locks = (msgs) => msgs.filter((m) => m.t === 'caps_lock').map((m) => `${m.on}/${m.num}`);
  let from = await mark(page);
  await page.evaluate(() => { window.__caps = true; window.__num = true; });
  await synthPointer(page, 'pointerdown', { pointerType: 'touch', isPrimary: true, pointerId: 9, button: 0, buttons: 1 });
  await synthPointer(page, 'pointerdown', { pointerType: 'mouse', isPrimary: true, pointerId: 1, button: 0, buttons: 1 });
  await settle(page);
  let got = locks(await since(page, from));
  record('15 pointer and touch events send no lock state', got.length === 0, `caps_lock sent: ${JSON.stringify(got)}`);
  from = await mark(page);
  await page.keyboard.press('KeyQ');
  await page.keyboard.press('KeyW');
  await settle(page);
  got = locks(await since(page, from));
  record('15b the next key sends Caps and Num Lock once', JSON.stringify(got) === '["true/true"]', `caps_lock sent: ${JSON.stringify(got)}`);
  await context.close();
}

// Mouse buttons follow `buttons`: a second button pressed and let go while the first is held
// (moves, not pointerdown/up), and a cancel, leave nothing held.
async function caseChordedButtons(browser, server) {
  const { context, page } = await openPage(browser, server);
  const cdp = await context.newCDPSession(page);
  const from = await mark(page);
  const mouse = (type, button, buttons) => cdp.send('Input.dispatchMouseEvent', { type, x: 600, y: 300, button, buttons, clickCount: 1 });
  await mouse('mousePressed', 'left', 1);
  await mouse('mousePressed', 'right', 3);
  await mouse('mouseReleased', 'left', 2);
  await mouse('mouseReleased', 'right', 0);
  await settle(page);
  const msgs = await since(page, from);
  const btns = msgs.filter((m) => m.t === 'pointer' && (m.action === 'press' || m.action === 'release')).map((m) => `${m.button}${m.action === 'press' ? '↓' : '↑'}`).join(' ');
  const model = await modelOf(page);
  record('16 chorded buttons: "1↓ 2↓ 1↑ 2↑", nothing held', btns === '1↓ 2↓ 1↑ 2↑' && !model.buttons.size, `sent ${btns}`, msgs);
  // A press, then the page hides mid-drag: the server lets go; the late release sends nothing.
  const at = await mark(page);
  await mouse('mousePressed', 'left', 1);
  await page.evaluate(() => { window.__hidden = true; document.dispatchEvent(new Event('visibilitychange')); });
  await page.evaluate(() => { window.__hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
  await mouse('mouseReleased', 'left', 0);
  await settle(page);
  const late = (await since(page, at)).filter((m) => m.t === 'pointer' && m.action !== 'motion').map((m) => `${m.button}${m.action === 'press' ? '↓' : '↑'}`).join(' ');
  const model2 = await modelOf(page);
  const held = await page.evaluate(() => buttonsHeld.size);
  record('16b hidden mid-drag: the server let go, the late release sends nothing', late === '1↓' && !model2.buttons.size && !held, `sent ${late}`);
  await context.close();
}

// One motion message per move event; wheel notches as steps, a touchpad's deltas as pixels.
async function caseMotionAndWheel(browser, server) {
  const { context, page } = await openPage(browser, server);
  const cdp = await context.newCDPSession(page);
  let from = await mark(page);
  for (let i = 0; i < 10; i++) await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 300 + i * 7, y: 300, buttons: 0 });
  await settle(page);
  const moves = (await since(page, from)).filter((m) => m.t === 'pointer' && m.action === 'motion').length;
  record('17 ten mouse moves: at most ten motion messages', moves >= 1 && moves <= 10, `${moves} motion messages`);
  from = await mark(page);
  const wheel = (init) => page.evaluate((i) => document.activeElement.dispatchEvent(new WheelEvent('wheel', { bubbles: true, cancelable: true, clientX: 300, clientY: 300, ...i })), init);
  await wheel({ deltaY: 100, wheelDeltaY: -120 });
  await wheel({ deltaY: 3, deltaMode: 1 });
  await wheel({ deltaY: 7.5, wheelDeltaY: -9 });
  await settle(page);
  const sc = (await since(page, from)).filter((m) => m.t === 'scroll');
  const ok = sc.length === 3 && sc[0].dy === 1 && !sc[0].pixels && sc[1].dy === 1 && !sc[1].pixels && sc[2].pixels === true && Math.abs(sc[2].dy - 7.5) < 0.5;
  record('18 wheel: a notch and a line are steps, a touchpad delta is pixels', ok, JSON.stringify(sc.map((m) => [m.dy, !!m.pixels])));
  await context.close();
}

// Sizes: a rotation sends each new size once, with the screen turned; a frame of the old size
// that was on its way is acknowledged, not drawn; fullscreenchange and visibility send no resize
// when nothing changed.
async function caseRotation(browser, server) {
  const { context, page } = await openPage(browser, server);
  const cdp = await context.newCDPSession(page);
  const metrics = (w, h) => cdp.send('Emulation.setDeviceMetricsOverride', { width: w, height: h, deviceScaleFactor: 1, mobile: false, screenWidth: w, screenHeight: h });
  const before = await page.evaluate(() => windows.get(1).size.slice());
  let from = await mark(page);
  await metrics(720, 1280);
  await page.waitForTimeout(300);
  let msgs = (await since(page, from)).filter((m) => m.t === 'resize');
  const stage = await page.evaluate(() => { const r = document.getElementById('stage').getBoundingClientRect(); return [Math.round(r.width), Math.round(r.height)]; });
  const last = msgs[msgs.length - 1];
  const okRot = msgs.length === 1 && last.width === stage[0] && last.height === stage[1] && last.screen && last.screen[0] === 720 && last.screen[1] === 1280;
  record('19 rotation: one resize, the stage size, the screen turned', okRot, JSON.stringify(msgs.map((m) => [m.width, m.height, m.screen])), msgs);
  // The frame the server sent before it heard: the old size. Acknowledged, not drawn.
  from = await mark(page);
  await page.evaluate(() => {
    window.__drawn = [];
    const post = drawer.post;
    drawer.post = (m, t) => { window.__drawn.push(m.t); return post(m, t); };
  });
  await page.evaluate(([w, h]) => window.__deliver({ t: 'tiles', win: 1, width: w, height: h, rects: [[0, 0, w, h]], len: 0 }), before);
  await settle(page);
  msgs = await since(page, from);
  const drawn = await page.evaluate(() => window.__drawn);
  record('19b a frame of the old size is acknowledged, not drawn', msgs.some((m) => m.t === 'frame_done' && m.win === 1) && !drawn.includes('tiles'), `drawer got ${JSON.stringify(drawn)}, sent ${msgs.map((m) => m.t)}`);
  from = await mark(page);
  await page.evaluate(() => { document.dispatchEvent(new Event('fullscreenchange')); window.__hidden = true; document.dispatchEvent(new Event('visibilitychange')); window.__hidden = false; document.dispatchEvent(new Event('visibilitychange')); });
  await page.waitForTimeout(600);
  msgs = (await since(page, from)).filter((m) => m.t === 'resize');
  record('19c fullscreenchange and hide/show with the same size: no resize', msgs.length === 0, `${msgs.length} resizes`);
  await context.close();
}

// ------------------------------------------------------------------ stress

// mulberry32: a seeded generator, so a failing storm runs again with its seed.
function rng(seed) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6D2B79F5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  return { next, below: (n) => Math.floor(next() * n), pick: (a_) => a_[Math.floor(next() * a_.length)] };
}

const STRESS_KEYS = ['KeyA', 'KeyB', 'KeyS', 'Digit1', 'Enter', 'Backspace', 'ArrowLeft', 'Space',
  'ShiftLeft', 'ShiftRight', 'ControlLeft', 'AltLeft', 'AltRight', 'MetaLeft'];
const BUTTONS = { left: 1, right: 2, middle: 4 };

// One storm: random keys (down, auto-repeat, up, stray ups), IME key events (229), modifier
// chords and flags that disagree with the keys held (lost key-ups), mouse presses, moves and
// releases (chorded), blur and focus, hidden and shown, rotations and resizes, and the link
// closing and coming back. Then everything is let go, the page is shown and focused, a key is
// typed, and the page and the server's side are checked:
// - nothing held: the page's own keys and buttons, and the server's (from the messages);
// - no press for a button already down, no repeat of a modifier (a key down again is a repeat);
// - the last size sent is the stage's, with the screen the viewport's;
// - the last lock state sent is the keyboard's.
async function storm(browser, server, seed, iterations) {
  const r = rng(seed);
  const { context, page } = await openPage(browser, server);
  const cdp = await context.newCDPSession(page);
  let vw = 1280, vh = 720;
  const keys = new Set(); // what Playwright holds down
  const buttons = new Set();
  let mx = 400, my = 300, hidden = false;
  const steps = [];
  const mouse = (type, button) => cdp.send('Input.dispatchMouseEvent', {
    type, x: mx, y: my, button: button || 'none', buttons: [...buttons].reduce((m, b) => m | BUTTONS[b], 0), clickCount: 1,
  });
  const synth = (type, init) => page.evaluate(([t, i, k229]) => {
    const e = new KeyboardEvent(t, { bubbles: true, cancelable: true, ...i });
    if (k229) Object.defineProperty(e, 'keyCode', { get: () => 229 });
    (document.activeElement || document.body).dispatchEvent(e);
  }, [type, init, !!init.isComposing]);
  const setHidden = async (h) => {
    hidden = h;
    await page.evaluate((x) => { window.__hidden = x; document.dispatchEvent(new Event('visibilitychange')); }, h);
  };
  try {
    for (let n = 0; n < iterations; n++) {
      const roll = r.below(100);
      if (roll < 30) {
        const k = r.pick(STRESS_KEYS);
        steps.push(`down ${k}${keys.has(k) ? ' (repeat)' : ''}`);
        keys.add(k);
        await page.keyboard.down(k);
      } else if (roll < 50) {
        const k = keys.size && r.below(4) ? r.pick([...keys]) : r.pick(STRESS_KEYS);
        steps.push(`up ${k}`);
        keys.delete(k);
        await page.keyboard.up(k);
      } else if (roll < 55) {
        // The IME: a key-up in a composition naming a key (held or not), or a composing key-down.
        const k = r.pick(['KeyA', 'KeyB', 'KeyS', 'Enter']);
        const type = r.below(2) ? 'keyup' : 'keydown';
        steps.push(`ime ${type} ${k}`);
        await synth(type, { code: k, key: 'Process', isComposing: true });
      } else if (roll < 60) {
        // A key whose flags disagree with what is held: lost key-ups, or flags with no key.
        const flags = { shiftKey: !!r.below(2), ctrlKey: !r.below(4), altKey: !r.below(5), metaKey: !r.below(6) };
        steps.push(`flags ${JSON.stringify(flags)}`);
        await synth('keydown', { code: 'KeyZ', key: 'z', ...flags });
        await synth('keyup', { code: 'KeyZ', key: 'z', ...flags });
      } else if (roll < 63) {
        const lock = r.below(2) ? 'CapsLock' : 'NumLock';
        steps.push(`toggle ${lock}`);
        await page.evaluate((l) => { if (l === 'CapsLock') window.__caps = !window.__caps; else window.__num = !window.__num; }, lock);
        await page.keyboard.press(lock);
      } else if (roll < 73) {
        const b = r.pick(Object.keys(BUTTONS));
        if (buttons.has(b)) { buttons.delete(b); steps.push(`release ${b}`); await mouse('mouseReleased', b); }
        else { buttons.add(b); steps.push(`press ${b}`); await mouse('mousePressed', b); }
      } else if (roll < 83) {
        mx = 10 + r.below(vw - 20);
        my = 50 + r.below(vh - 70);
        steps.push(`move ${mx},${my}`);
        await mouse('mouseMoved');
      } else if (roll < 87) {
        const ev = r.below(2) ? 'blur' : 'focus';
        steps.push(ev);
        await page.evaluate((e) => window.dispatchEvent(new Event(e)), ev);
      } else if (roll < 92) {
        steps.push(hidden ? 'shown' : 'hidden');
        await setHidden(!hidden);
      } else if (roll < 97) {
        [vw, vh] = r.below(3) ? [vh, vw] : [600 + r.below(800), 500 + r.below(600)];
        mx = Math.min(mx, vw - 10); my = Math.min(my, vh - 10);
        steps.push(`viewport ${vw}x${vh}`);
        await cdp.send('Emulation.setDeviceMetricsOverride', { width: vw, height: vh, deviceScaleFactor: 1, mobile: false, screenWidth: vw, screenHeight: vh });
        if (r.below(2)) await page.waitForTimeout(20);
      } else {
        steps.push('link closed');
        if (hidden) await setHidden(false);
        await reconnect(page);
      }
    }
    // Settle: let go of everything, come back on screen with the focus, type one key.
    for (const k of [...keys]) await page.keyboard.up(k);
    for (const b of [...buttons]) { buttons.delete(b); await mouse('mouseReleased', b); }
    if (hidden) await setHidden(false);
    await page.evaluate(() => window.dispatchEvent(new Event('focus')));
    await page.keyboard.press('KeyM');
    await page.waitForTimeout(300);

    const sent = await page.evaluate(() => window.__sent);
    const model = new SeatModel().feed(sent);
    const pageHeld = await page.evaluate(() => ({ keys: [...held], buttons: typeof buttonsHeld === 'undefined' ? [] : [...buttonsHeld] }));
    const real = await page.evaluate(() => {
      const r_ = document.getElementById('stage').getBoundingClientRect(), d = devicePixelRatio || 1;
      return { width: Math.round(r_.width * d), height: Math.round(r_.height * d), screen: [Math.round(screen.width * d), Math.round(screen.height * d)], caps: window.__caps, num: window.__num };
    });
    const problems = [];
    if (pageHeld.keys.length || pageHeld.buttons.length) problems.push(`the page still holds ${JSON.stringify(pageHeld)}`);
    if (model.keys.size || model.buttons.size) problems.push(`the server still holds keys ${[...model.keys]} buttons ${[...model.buttons]}`);
    if (model.errors.length) problems.push(`presses of what was down: ${model.errors.slice(0, 5).join('; ')}${model.errors.length > 5 ? ` (+${model.errors.length - 5})` : ''}`);
    const sz = model.size;
    if (!sz || sz.width !== real.width || sz.height !== real.height || !sz.screen || sz.screen[0] !== real.screen[0] || sz.screen[1] !== real.screen[1]) {
      problems.push(`last size sent ${JSON.stringify(sz)}, the stage is ${real.width}x${real.height} on ${real.screen}`);
    }
    if (!model.locks || model.locks[0] !== real.caps || model.locks[1] !== real.num) problems.push(`last locks sent ${JSON.stringify(model.locks)}, the keyboard has caps ${real.caps} num ${real.num}`);
    const counts = sent.reduce((c, m) => { c[m.t] = (c[m.t] || 0) + 1; return c; }, {});
    record(`stress seed ${seed}: ${iterations} events`, !problems.length,
      problems.length ? `${problems.join('; ')}\n  last steps: ${steps.slice(-15).join(' | ')}` : `sent ${Object.entries(counts).map(([k, v]) => `${v} ${k}`).join(', ')}`,
      problems.length ? sent.slice(-60) : undefined);
  } finally {
    await context.close();
  }
}

// ------------------------------------------------------------------ main

const stress = process.argv.includes('--stress');
const server = await serve();
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
try {
  console.log(`testing ${jsPath}`);
  if (stress) {
    const seed = Number(argOf('--seed') || 1), storms = Number(argOf('--storms') || 5), iterations = Number(argOf('--iterations') || 300);
    for (let k = 0; k < storms; k++) await storm(browser, server, seed + k, iterations);
  } else {
    for (const c of [caseBlur, caseHidden, caseReconnect, caseLostKeyUp, caseCaps, caseFlagShift, caseAltGr, caseTouchFlags,
      caseWindowSwitch, caseTwoShifts, caseMeta, caseRepeat, caseImeKeyUp, caseModifierPressedFromFlag, caseLocks,
      caseChordedButtons, caseMotionAndWheel, caseRotation]) {
      await c(browser, server);
    }
  }
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
