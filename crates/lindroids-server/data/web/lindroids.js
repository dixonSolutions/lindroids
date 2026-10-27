// Lindroids in the browser: shows the Linux windows of the Lindroids server that served this page,
// and sends back touch, mouse, pen, keys and text.
//
// The page speaks the server's link protocol (crates/lindroids-proto) over the WebSocket at /link:
// each message from the server is one WebSocket message (a JSON line, then its payload, if any).
// It proves itself with the pairing secret, which `lindroids run` puts in the URL's fragment
// (#k=...), and keeps it in this origin's storage.
'use strict';

const BUILD = '@BUILD@';
const PROTOCOL_VERSION = 4;
const SECRET_KEY = 'lindroids.secret';

const $ = (id) => document.getElementById(id);
const tabsEl = $('tabs'), stage = $('stage'), panel = $('panel'), ime = $('ime'), toastsEl = $('toasts');
const enc = new TextEncoder(), dec = new TextDecoder();

let ws = null;
let ready = false;
let replaced = false;
let retry = null;
let myNonce = '';
let t0 = performance.now();
const windows = new Map(); // win -> {win, title, appId, icon, canvas, ctx, tab, pendingAck}
let active = null; // win
const order = []; // most recently active last

// ------------------------------------------------------------------ secret and auth

function takeSecretFromUrl() {
  const m = /[#&]k=([^&]+)/.exec(location.hash);
  if (!m) return false;
  localStorage.setItem(SECRET_KEY, decodeURIComponent(m[1]));
  // Keep the secret out of the address bar, history and bookmarks.
  history.replaceState(null, '', location.pathname + location.search);
  return true;
}
takeSecretFromUrl();

// `lindroids run` opens the page again to bring it to the front; the browser reuses this tab and
// only the fragment changes. If this tab had lost the link (to another tab, or a server that
// restarted with a new secret), take it back now: the user asked for it.
window.addEventListener('hashchange', () => {
  if (!takeSecretFromUrl()) return;
  if (!ws || !ready) connect();
});

function b64url(buf) {
  let s = '';
  for (const b of new Uint8Array(buf)) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

async function mac(secret, label, nonce) {
  const key = await crypto.subtle.importKey('raw', enc.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const l = enc.encode(label), n = enc.encode(nonce);
  const data = new Uint8Array(l.length + 1 + n.length);
  data.set(l, 0);
  data.set(n, l.length + 1);
  return b64url(await crypto.subtle.sign('HMAC', key, data));
}

function randomNonce() {
  const a = new Uint8Array(24);
  crypto.getRandomValues(a);
  return b64url(a);
}

// ------------------------------------------------------------------ the link

function connect() {
  clearTimeout(retry);
  replaced = false;
  ready = false;
  const url = (location.protocol === 'https:' ? 'wss://' : 'ws://') + location.host + '/link';
  try {
    ws = new WebSocket(url);
  } catch (e) {
    return waitForServer();
  }
  ws.binaryType = 'arraybuffer';
  ws.onopen = () => {
    myNonce = randomNonce();
    const dpr = window.devicePixelRatio || 1;
    send({
      t: 'hello', version: PROTOCOL_VERSION, build: BUILD, density: dpr, nonce: myNonce,
      screen: [Math.round(screen.width * dpr), Math.round(screen.height * dpr)],
      page: location.pathname,
    });
  };
  ws.onmessage = (ev) => {
    const bytes = new Uint8Array(ev.data);
    const nl = bytes.indexOf(10);
    const msg = JSON.parse(dec.decode(nl < 0 ? bytes : bytes.subarray(0, nl)));
    const payload = nl < 0 ? new Uint8Array(0) : bytes.subarray(nl + 1);
    handle(msg, payload).catch((e) => console.error('lindroids:', msg.t, e));
  };
  ws.onclose = () => {
    ws = null;
    const was = ready;
    ready = false;
    for (const w of [...windows.keys()]) removeWindow(w);
    if (replaced) return;
    if (was) toast({ title: 'The server went away', body: 'Reconnecting…' }, 3000);
    waitForServer();
  };
}

function waitForServer() {
  const termux = /Android/.test(navigator.userAgent)
    ? `<p><a href="intent:#Intent;action=android.intent.action.MAIN;category=android.intent.category.LAUNCHER;package=com.termux;end"><button>Open Termux</button></a></p>` : '';
  showPanel(`<p>Waiting for the Lindroids server…</p>
    <p>In Termux, run <code>lindroids run &lt;program&gt;</code>; this page connects by itself.</p>${termux}`);
  retry = setTimeout(connect, 2000);
}

function send(msg, payload) {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  const line = enc.encode(JSON.stringify(msg) + '\n');
  if (!payload || !payload.length) return ws.send(line);
  const all = new Uint8Array(line.length + payload.length);
  all.set(line, 0);
  all.set(payload, line.length);
  ws.send(all);
}

const now = () => Math.round(performance.now() - t0) >>> 0;

async function handle(m, payload) {
  switch (m.t) {
    case 'challenge': {
      const secret = localStorage.getItem(SECRET_KEY);
      // A pairing code typed while disconnected goes in as soon as the server asks.
      if (pendingCode && !secret) {
        const code = pendingCode;
        pendingCode = null;
        return send({ t: 'auth', code });
      }
      if (secret) {
        if (m.proof === await mac(secret, 'lindroids-server', myNonce)) {
          return send({ t: 'auth', proof: await mac(secret, 'lindroids-app', m.nonce) });
        }
        // A secret for another server (or one from before `lindroids` made a new one).
        localStorage.removeItem(SECRET_KEY);
      }
      return askCode(m.nonce, '');
    }
    case 'auth_failed':
      if (!m.pairing_open) return showPanel(`<p class="err">Pairing is closed after too many wrong codes.</p>
        <p>Run <code>lindroids stop</code>, then <code>lindroids run &lt;program&gt;</code> again in Termux.</p>`);
      replaced = true; // reconnect by hand, with the code
      return askCode(null, m.reason);
    case 'paired':
      return localStorage.setItem(SECRET_KEY, m.secret);
    case 'hello':
      ready = true;
      hidePanelIfWindows();
      sendSystem();
      return;
    case 'bye':
      replaced = true;
      // A tab the browser opened for `lindroids run` that a newer one replaced: close it, where
      // the browser lets a page do that (one history entry), so tabs do not pile up.
      if (history.length <= 1) setTimeout(() => window.close(), 300);
      return showPanel(`<p>${esc(m.reason)}.</p><p><button id="again">Show the windows here</button></p>`, () => $('again').onclick = connect);
    case 'window_new': return newWindow(m);
    case 'window_title': {
      const w = windows.get(m.win);
      if (w) { w.title = m.title; w.tab.querySelector('span').textContent = m.title; if (active === m.win) document.title = m.title; }
      return;
    }
    case 'window_close': return removeWindow(m.win);
    case 'window_activate': return activate(m.win);
    case 'cursor': return stage.style.setProperty('--cursor', m.css);
    case 'tiles': return tiles(m, payload);
    case 'keyboard':
      if (m.win !== active) return;
      // Typing on a hardware keyboard: no soft keyboard, and moving the focus now would lose
      // the keys in flight.
      if (m.show && performance.now() - lastHardwareKey < 3000) return;
      return m.show ? showKeyboard() : hideKeyboard();
    case 'clipboard': return clipboardFromLinux(m.text);
    case 'audio_open': return audioOpen(m);
    case 'audio_data': return audioData(m, payload);
    case 'audio_cork': return;
    case 'audio_close': return streams.delete(m.stream);
    case 'open_uri': return openUri(m);
    case 'open_file': return openFile(m, payload);
    case 'notify': return notify(m);
    case 'close_notification': return closeToast('n:' + m.id);
    case 'pick_files': return pickFiles(m);
    case 'fetch_file': return fetchFile(m);
    case 'write_file': return writeFile(m, payload);
    case 'pick_folder': return send({ t: 'folder_picked', request: m.request, path: null });
    case 'log': return console.log('server', m.level, m.msg);
    default: return;
  }
}

function askCode(nonce, err) {
  showPanel(`<p>Enter the pairing code that <code>lindroids pair</code> shows in Termux.</p>
    ${err ? `<p class="err">${esc(err)}</p>` : ''}
    <p><input id="code" inputmode="numeric" autocomplete="one-time-code" maxlength="6"> <button id="go">Pair</button></p>`, () => {
    const go = () => {
      const code = $('code').value.trim();
      if (!code) return;
      if (nonce && ws) send({ t: 'auth', code });
      else { pendingCode = code; connect(); }
    };
    $('go').onclick = go;
    $('code').onkeydown = (e) => { if (e.key === 'Enter') go(); };
    $('code').focus();
  });
}
let pendingCode = null;

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

function showPanel(html, after) {
  panel.hidden = false;
  panel.innerHTML = `<div class="box"><img src="/icon.svg" alt="">${html}</div>`;
  if (after) after();
}

function hidePanelIfWindows() {
  if (!ready) return;
  if (windows.size) { panel.hidden = true; return; }
  showPanel(`<p>Connected. No windows yet.</p><p>Run a program in Termux: <code>lindroids run &lt;program&gt;</code></p>`);
}

// ------------------------------------------------------------------ windows

function newWindow(m) {
  if (windows.has(m.win)) removeWindow(m.win);
  const canvas = document.createElement('canvas');
  canvas.tabIndex = -1;
  stage.appendChild(canvas);
  const tab = document.createElement('div');
  tab.className = 'tab';
  tab.innerHTML = `${m.icon_png_b64 ? `<img alt="" src="data:image/png;base64,${m.icon_png_b64}">` : ''}<span></span><button title="Close">×</button>`;
  tab.querySelector('span').textContent = m.title;
  tab.onclick = () => activate(m.win);
  tab.querySelector('button').onclick = (e) => { e.stopPropagation(); send({ t: 'close', win: m.win }); };
  tabsEl.appendChild(tab);
  const w = { win: m.win, title: m.title, appId: m.app_id, icon: m.icon_png_b64, canvas, ctx: canvas.getContext('2d'), tab, pendingAck: false, size: [0, 0] };
  windows.set(m.win, w);
  attachInput(w);
  resizeWindow(w);
  activate(m.win);
}

function removeWindow(win) {
  const w = windows.get(win);
  if (!w) return;
  windows.delete(win);
  w.canvas.remove();
  w.tab.remove();
  const i = order.indexOf(win);
  if (i >= 0) order.splice(i, 1);
  if (active === win) {
    active = null;
    const next = order[order.length - 1];
    if (next !== undefined) activate(next);
    else { document.title = 'Lindroids'; hideKeyboard(); }
  }
  hidePanelIfWindows();
}

function activate(win) {
  const w = windows.get(win);
  if (!w) return;
  if (active !== null && active !== win) {
    const old = windows.get(active);
    if (old) { old.canvas.classList.remove('active'); old.tab.classList.remove('active'); }
    send({ t: 'focus', win: active, focused: false });
  }
  active = win;
  const i = order.indexOf(win);
  if (i >= 0) order.splice(i, 1);
  order.push(win);
  w.canvas.classList.add('active');
  w.tab.classList.add('active');
  w.tab.scrollIntoView({ inline: 'nearest', block: 'nearest' });
  document.title = w.title || 'Lindroids';
  panel.hidden = true;
  send({ t: 'focus', win, focused: document.hasFocus() || !document.hidden });
  ackLater(w);
}

/// The size every window gets: the stage, in physical pixels.
function stageSize() {
  const r = stage.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  return [Math.max(1, Math.round(r.width * dpr)), Math.max(1, Math.round(r.height * dpr)), dpr];
}

function resizeWindow(w) {
  // Not laid out yet (a page loading behind the browser's own screens): wait for a real size.
  const r = stage.getBoundingClientRect();
  if (r.width < 2 || r.height < 2) return;
  const [width, height, density] = stageSize();
  if (w.size[0] === width && w.size[1] === height) return;
  w.size = [width, height];
  send({ t: 'resize', win: w.win, width, height, density });
}

new ResizeObserver(() => { for (const w of windows.values()) resizeWindow(w); }).observe(stage);

// ------------------------------------------------------------------ frames

function lz4(src, dst) {
  let s = 0, d = 0;
  const end = src.length;
  while (s < end) {
    const token = src[s++];
    let lit = token >> 4;
    if (lit === 15) { let b; do { b = src[s++]; lit += b; } while (b === 255); }
    dst.set(src.subarray(s, s + lit), d);
    s += lit; d += lit;
    if (s >= end) break;
    const offset = src[s] | (src[s + 1] << 8);
    s += 2;
    let len = token & 15;
    if (len === 15) { let b; do { b = src[s++]; len += b; } while (b === 255); }
    len += 4;
    let m = d - offset;
    if (offset >= len) { dst.copyWithin(d, m, m + len); d += len; }
    else { for (let k = 0; k < len; k++) dst[d++] = dst[m++]; }
  }
  return d;
}

function tiles(m, payload) {
  const w = windows.get(m.win);
  if (!w) return send({ t: 'frame_done', win: m.win });
  if (w.canvas.width !== m.width || w.canvas.height !== m.height) {
    w.canvas.width = m.width;
    w.canvas.height = m.height;
  }
  const size = payload[0] | (payload[1] << 8) | (payload[2] << 16) | (payload[3] << 24);
  const raw = new Uint8ClampedArray(size);
  lz4(payload.subarray(4), raw);
  let off = 0;
  for (const [x, y, rw, rh] of m.rects) {
    const n = rw * rh * 4;
    w.ctx.putImageData(new ImageData(raw.subarray(off, off + n), rw, rh), x, y);
    off += n;
  }
  w.pendingAck = true;
  ackLater(w);
}

/// Acknowledge a frame once the screen can take the next one. A window that is not shown holds
/// its acknowledgement, so its program stops drawing until it is.
function ackLater(w) {
  if (!w.pendingAck || w.win !== active || document.hidden || w.ackQueued) return;
  w.ackQueued = true;
  requestAnimationFrame(() => {
    w.ackQueued = false;
    if (!w.pendingAck || !windows.has(w.win)) return;
    w.pendingAck = false;
    send({ t: 'frame_done', win: w.win });
  });
}

document.addEventListener('visibilitychange', () => {
  const w = windows.get(active);
  if (!w) return;
  send({ t: 'focus', win: w.win, focused: !document.hidden });
  ackLater(w);
});

// ------------------------------------------------------------------ pointer, touch, pen

function toPhys(w, e) {
  const r = w.canvas.getBoundingClientRect();
  return [(e.clientX - r.left) * (w.canvas.width / r.width), (e.clientY - r.top) * (w.canvas.height / r.height)];
}

const touchIds = new Map(); // pointerId -> small touch id

function attachInput(w) {
  const c = w.canvas;
  const penDown = new Set();
  c.addEventListener('contextmenu', (e) => e.preventDefault());
  c.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    resumeAudio();
    if (e.pointerType !== 'touch') c.focus({ preventScroll: true });
    c.setPointerCapture(e.pointerId);
    const [x, y] = toPhys(w, e);
    if (e.pointerType === 'touch') {
      let id = 0;
      while ([...touchIds.values()].includes(id)) id++;
      touchIds.set(e.pointerId, id);
      send({ t: 'touch', win: w.win, action: 'down', id, x, y, time_ms: now() });
    } else {
      // A pen acts as a mouse with its button held: it draws in note and paint programs.
      const button = e.pointerType === 'pen' ? (e.button === 2 || e.button === 5 ? 2 : 1) : mouseButton(e.button);
      if (!button) return;
      if (e.pointerType === 'pen') penDown.add(e.pointerId);
      send({ t: 'pointer', win: w.win, action: 'motion', x, y, button: 0, time_ms: now() });
      send({ t: 'pointer', win: w.win, action: 'press', x, y, button, time_ms: now() });
    }
  });
  c.addEventListener('pointermove', (e) => {
    const events = e.getCoalescedEvents ? e.getCoalescedEvents() : [e];
    for (const ev of events.length ? events : [e]) {
      const [x, y] = toPhys(w, ev);
      if (e.pointerType === 'touch') {
        const id = touchIds.get(e.pointerId);
        if (id !== undefined) send({ t: 'touch', win: w.win, action: 'move', id, x, y, time_ms: now() });
      } else {
        send({ t: 'pointer', win: w.win, action: 'motion', x, y, button: 0, time_ms: now() });
      }
    }
  });
  const up = (e, cancel) => {
    const [x, y] = toPhys(w, e);
    if (e.pointerType === 'touch') {
      const id = touchIds.get(e.pointerId);
      if (id === undefined) return;
      touchIds.delete(e.pointerId);
      send({ t: 'touch', win: w.win, action: cancel ? 'cancel' : 'up', id, x, y, time_ms: now() });
    } else if (e.pointerType === 'pen') {
      if (!penDown.delete(e.pointerId)) return;
      send({ t: 'pointer', win: w.win, action: 'release', x, y, button: e.button === 2 || e.button === 5 ? 2 : 1, time_ms: now() });
    } else {
      const button = mouseButton(e.button);
      if (button) send({ t: 'pointer', win: w.win, action: 'release', x, y, button, time_ms: now() });
    }
  };
  c.addEventListener('pointerup', (e) => up(e, false));
  c.addEventListener('pointercancel', (e) => up(e, true));
  c.addEventListener('pointerenter', (e) => {
    if (e.pointerType === 'touch') return;
    const [x, y] = toPhys(w, e);
    send({ t: 'pointer', win: w.win, action: 'enter', x, y, button: 0, time_ms: now() });
  });
  c.addEventListener('pointerleave', (e) => {
    if (e.pointerType === 'touch' || e.buttons) return;
    const [x, y] = toPhys(w, e);
    send({ t: 'pointer', win: w.win, action: 'leave', x, y, button: 0, time_ms: now() });
  });
  c.addEventListener('wheel', (e) => {
    e.preventDefault();
    // Wheel notches: Chrome and Firefox report 100 px (or 3 lines) per notch.
    const k = e.deltaMode === 1 ? 1 / 3 : e.deltaMode === 2 ? 1 : 1 / 100;
    send({ t: 'scroll', win: w.win, dx: e.deltaX * k, dy: e.deltaY * k, time_ms: now() });
  }, { passive: false });
}

function mouseButton(b) {
  return { 0: 1, 1: 3, 2: 2, 3: 4, 4: 5 }[b] || 0;
}

// ------------------------------------------------------------------ keys and text

// KeyboardEvent.code -> Android KeyEvent keycode (the server maps those to evdev).
const CODES = (() => {
  const m = { Space: 62, Enter: 66, NumpadEnter: 160, Backspace: 67, Delete: 112, Tab: 61, Escape: 111,
    ShiftLeft: 59, ShiftRight: 60, ControlLeft: 113, ControlRight: 114, AltLeft: 57, AltRight: 58,
    MetaLeft: 117, MetaRight: 118, OSLeft: 117, OSRight: 118, ArrowUp: 19, ArrowDown: 20, ArrowLeft: 21, ArrowRight: 22,
    Home: 122, End: 123, PageUp: 92, PageDown: 93, Insert: 124, Comma: 55, Period: 56, Minus: 69, Equal: 70,
    BracketLeft: 71, BracketRight: 72, Backslash: 73, Semicolon: 74, Quote: 75, Slash: 76, Backquote: 68,
    CapsLock: 115, NumLock: 143, ScrollLock: 116, PrintScreen: 120, Pause: 121, ContextMenu: 82,
    NumpadDivide: 154, NumpadMultiply: 155, NumpadSubtract: 156, NumpadAdd: 157, NumpadDecimal: 158,
    NumpadComma: 159, NumpadEqual: 161 };
  for (let i = 0; i < 26; i++) m['Key' + String.fromCharCode(65 + i)] = 29 + i;
  for (let i = 0; i < 10; i++) { m['Digit' + i] = 7 + i; m['Numpad' + i] = 144 + i; }
  for (let i = 1; i <= 12; i++) m['F' + i] = 130 + i;
  return m;
})();
// Soft keyboards often leave `code` empty: go by `key` for the keys that are not text.
const KEYS = { Enter: 66, Backspace: 67, Delete: 112, Tab: 61, Escape: 111, ArrowUp: 19, ArrowDown: 20,
  ArrowLeft: 21, ArrowRight: 22, Home: 122, End: 123, PageUp: 92, PageDown: 93 };

function keyCode(e) {
  if (e.isComposing || e.keyCode === 229) return 0;
  return CODES[e.code] || KEYS[e.key] || 0;
}

const held = new Set();
let pasteWait = null;
let lastHardwareKey = -1e9;

function onKey(e, down) {
  if (active === null || !ready || e.target.tagName === 'INPUT') return;
  const code = keyCode(e);
  if (!code) return;
  if (e.code) lastHardwareKey = performance.now();
  // Ctrl+V (and Ctrl+Shift+V): let the browser's paste through first, so the Linux side has the
  // text when the keys arrive.
  if (down && code === 50 && (e.ctrlKey || e.metaKey) && !pasteWait) {
    pasteWait = setTimeout(() => { pasteWait = null; key(code, true); key(code, false); }, 150);
    return;
  }
  if (!down && code === 50 && pasteWait) return;
  e.preventDefault();
  // Some keyboards (virtual ones, Android's injected keys) say Shift only as a flag on the
  // letter, with no Shift key of its own: press one around it, or "A" arrives as "a".
  const shiftHeld = held.has(59) || held.has(60);
  if (down && needsShift(e) && !shiftHeld && code !== 59 && code !== 60) {
    key(59, true);
    synthShift.add(code);
  }
  if (down) held.add(code); else held.delete(code);
  key(code, down);
  if (!down && synthShift.delete(code)) key(59, false);
}
const synthShift = new Set();

/// Whether the character typed needs Shift on a US keyboard (the keymap the Linux side has).
function needsShift(e) {
  if (e.shiftKey) return true;
  if (e.key.length !== 1 || e.ctrlKey || e.altKey || e.metaKey) return false;
  if (/^[A-Z]$/.test(e.key)) return !e.getModifierState('CapsLock');
  return '~!@#$%^&*()_+{}|:"<>?'.includes(e.key);
}

function key(code, down) {
  send({ t: 'key', win: active, code, down, time_ms: now() });
}

document.addEventListener('keydown', (e) => onKey(e, true));
document.addEventListener('keyup', (e) => onKey(e, false));
window.addEventListener('blur', () => {
  // Keys held while the page lost focus would stay down on the Linux side.
  for (const c of held) key(c, false);
  held.clear();
});
document.addEventListener('paste', (e) => {
  const text = e.clipboardData && e.clipboardData.getData('text/plain');
  if (text) { lastClipboard = text; send({ t: 'clipboard', text }); }
  if (pasteWait) {
    clearTimeout(pasteWait);
    pasteWait = null;
    e.preventDefault();
    key(50, true);
    key(50, false);
  }
});

// The soft keyboard types into a hidden text area; what changes in it becomes text and
// backspaces for the Linux side. Diffing (instead of trusting each event) copes with predictive
// keyboards that rewrite whole words.
const IME_BASE = '  ';
let imeLast = IME_BASE;
let composing = false;

function imeReset() {
  ime.value = IME_BASE;
  imeLast = IME_BASE;
  ime.setSelectionRange(IME_BASE.length, IME_BASE.length);
}

function showKeyboard() {
  // Already up: resetting what is being typed would lose it.
  if (document.activeElement === ime) return;
  imeReset();
  ime.focus({ preventScroll: true });
}

function hideKeyboard() {
  if (document.activeElement === ime) ime.blur();
}

ime.addEventListener('compositionstart', () => { composing = true; });
ime.addEventListener('compositionend', () => { composing = false; imeInput(); });
ime.addEventListener('input', () => imeInput());

function imeInput() {
  if (active === null) return imeReset();
  const now_ = ime.value;
  let p = 0;
  while (p < imeLast.length && p < now_.length && imeLast[p] === now_[p]) p++;
  const removed = imeLast.length - p;
  const added = now_.slice(p);
  for (let i = 0; i < removed; i++) { key(67, true); key(67, false); }
  // Enter as a key, so single-line fields activate.
  const parts = added.split('\n');
  parts.forEach((s, i) => {
    if (i > 0) { key(66, true); key(66, false); }
    if (s) send({ t: 'text', win: active, text: s.replace(/ /g, ' ') });
  });
  imeLast = now_;
  if (!composing && (now_.length > 64 || now_.length < IME_BASE.length || added.includes('\n'))) imeReset();
}

$('kb').onclick = () => {
  if (document.activeElement === ime) hideKeyboard();
  else showKeyboard();
};
$('fs').onclick = () => {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen({ navigationUI: 'hide' }).catch(() => {});
};

// ------------------------------------------------------------------ clipboard

let lastClipboard = null;

function clipboardFromLinux(text) {
  lastClipboard = text;
  if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).catch(() => {});
}

/// Hand the browser's clipboard to the Linux side, if it changed. Reading it needs the browser's
/// permission (asked once, on a tap) or a tap right now.
async function clipboardToLinux() {
  if (!ready || !navigator.clipboard || !navigator.clipboard.readText) return false;
  try {
    const text = await navigator.clipboard.readText();
    if (text && text !== lastClipboard) {
      lastClipboard = text;
      send({ t: 'clipboard', text });
    }
    return true;
  } catch (e) {
    return false;
  }
}

// Coming back to the page (from another Android app, where something may have been copied):
// sync without asking, when the browser already allows it.
async function clipboardOnReturn() {
  try {
    const p = await navigator.permissions.query({ name: 'clipboard-read' });
    if (p.state === 'granted') clipboardToLinux();
  } catch (e) {
    // Firefox and others do not say: they only allow it on a tap.
  }
}
window.addEventListener('focus', clipboardOnReturn);
document.addEventListener('visibilitychange', () => { if (!document.hidden) clipboardOnReturn(); });

// 📋: paste on a phone, which has no Ctrl+V. The tap lets the browser read the clipboard.
$('paste').onclick = async () => {
  if (active === null) return;
  if (!(await clipboardToLinux())) return toast({ title: 'The browser did not share its clipboard', body: 'Allow it when asked, or use Ctrl+V.' }, 5000);
  key(113, true); key(50, true); key(50, false); key(113, false);
};

// ------------------------------------------------------------------ sound

let audio = null;
const streams = new Map();

function resumeAudio() {
  if (audio && audio.state === 'suspended') audio.resume();
}

function audioOpen(m) {
  if (!audio) {
    const AC = window.AudioContext || window.webkitAudioContext;
    if (!AC) return;
    audio = new AC({ latencyHint: 'interactive' });
  }
  streams.set(m.stream, { rate: m.rate, channels: m.channels, format: m.format, next: 0 });
}

function audioData(m, payload) {
  const s = streams.get(m.stream);
  const credit = () => send({ t: 'audio_credit', stream: m.stream, bytes: m.len });
  if (!s || !audio || audio.state !== 'running') return credit();
  const bytes = payload.slice().buffer; // aligned copy
  const samples = s.format === 'f32le' ? new Float32Array(bytes) : new Int16Array(bytes);
  const scale = s.format === 'f32le' ? 1 : 1 / 32768;
  const frames = Math.floor(samples.length / s.channels);
  if (!frames) return credit();
  const buf = audio.createBuffer(s.channels, frames, s.rate);
  for (let ch = 0; ch < s.channels; ch++) {
    const out = buf.getChannelData(ch);
    for (let i = 0; i < frames; i++) out[i] = samples[i * s.channels + ch] * scale;
  }
  const src = audio.createBufferSource();
  src.buffer = buf;
  src.connect(audio.destination);
  const at = Math.max(s.next, audio.currentTime + 0.03);
  src.start(at);
  s.next = at + buf.duration;
  src.onended = credit;
}

// ------------------------------------------------------------------ Android's side of the desktop

function toast(n, timeout) {
  const el = document.createElement('div');
  el.className = 'toast';
  el.innerHTML = `${n.app ? `<small>${esc(n.app)}</small>` : ''}<b>${esc(n.title)}</b>${n.body ? `<div>${esc(n.body)}</div>` : ''}`;
  for (const [k, label] of n.actions || []) {
    const b = document.createElement('button');
    b.textContent = label;
    b.onclick = () => { n.onAction && n.onAction(k); el.remove(); };
    el.appendChild(b);
  }
  if (n.key) el.dataset.key = n.key;
  el.onclick = (e) => { if (e.target === el) { n.onAction && n.onAction('default'); el.remove(); n.onClose && n.onClose(2); } };
  if (n.key) closeToast(n.key);
  toastsEl.appendChild(el);
  if (timeout) setTimeout(() => { if (el.isConnected) { el.remove(); n.onClose && n.onClose(1); } }, timeout);
  return el;
}

function closeToast(key) {
  for (const el of toastsEl.querySelectorAll('.toast')) if (el.dataset.key === key) el.remove();
}

function notify(n) {
  toast({
    key: 'n:' + n.id, app: n.app, title: n.title, body: n.body, actions: n.actions,
    onAction: (a) => send({ t: 'notification_action', id: n.id, action: a }),
    onClose: (r) => send({ t: 'notification_closed', id: n.id, reason: r }),
  }, n.urgency >= 2 ? 0 : 8000);
}

function activeGesture() {
  return navigator.userActivation ? navigator.userActivation.isActive : false;
}

function openUri(m) {
  const go = () => window.open(m.uri, '_blank', 'noopener');
  if (activeGesture()) go();
  else toast({ title: 'Open link?', body: m.uri, actions: [['open', 'Open']], onAction: go }, 15000);
  send({ t: 'open_result', request: m.request, ok: true });
}

function openFile(m, payload) {
  const blob = new Blob([payload.slice()], { type: m.mime || 'application/octet-stream' });
  const url = URL.createObjectURL(blob);
  toast({ title: m.name, body: 'From a Linux program', actions: [['open', 'Open'], ['save', 'Save']],
    onAction: (a) => (a === 'save' ? download(url, m.name) : window.open(url, '_blank')) }, 30000);
  send({ t: 'open_result', request: m.request, ok: true });
}

function download(url, name) {
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
}

function dialog(html, buttons) {
  const el = document.createElement('div');
  el.className = 'dialog';
  el.innerHTML = html;
  const row = document.createElement('div');
  for (const [label, fn, ghost] of buttons) {
    const b = document.createElement('button');
    b.textContent = label;
    if (ghost) b.className = 'ghost';
    b.onclick = () => { el.remove(); fn(); };
    row.appendChild(b);
  }
  el.appendChild(row);
  document.body.appendChild(el);
  return el;
}

const picked = new Map(); // request -> [File]
const saves = new Map(); // uri -> {name, parts}

function pickFiles(p) {
  const cancel = () => send({ t: 'files_picked', request: p.request, files: [] });
  if (p.directory) return cancel();
  if (p.save) {
    const name = p.name || 'file';
    return dialog(`<p>${esc(p.title || 'Save')}</p><p><b>${esc(name)}</b> goes to the browser's downloads.</p>`, [
      ['Save', () => {
        const uri = 'save:' + p.request;
        saves.set(uri, { name, parts: [] });
        send({ t: 'files_picked', request: p.request, files: [{ uri, name, size: 0 }] });
      }],
      ['Cancel', cancel, true],
    ]);
  }
  const input = document.createElement('input');
  input.type = 'file';
  input.multiple = !!p.multiple;
  if (p.mime_types && p.mime_types.length) input.accept = p.mime_types.join(',');
  input.onchange = () => {
    const files = [...input.files];
    picked.set(p.request, files);
    send({ t: 'files_picked', request: p.request, files: files.map((f, i) => ({ uri: `pick:${p.request}:${i}`, name: f.name, size: f.size, mime: f.type || undefined })) });
  };
  input.oncancel = cancel;
  // The file chooser needs a tap on this page.
  dialog(`<p>${esc(p.title || 'A Linux program wants a file')}</p>`, [['Choose…', () => input.click()], ['Cancel', cancel, true]]);
}

async function fetchFile(m) {
  const f = (picked.get(m.request) || [])[m.index];
  if (!f) return send({ t: 'file_chunk', request: m.request, index: m.index, last: true, error: 'no such file', len: 0 });
  const CHUNK = 1 << 20;
  for (let off = 0; off < f.size || off === 0; off += CHUNK) {
    const part = new Uint8Array(await f.slice(off, off + CHUNK).arrayBuffer());
    const last = off + CHUNK >= f.size;
    send({ t: 'file_chunk', request: m.request, index: m.index, last, len: part.length }, part);
    if (last) break;
  }
}

function writeFile(m, payload) {
  const s = saves.get(m.uri);
  if (!s) return send({ t: 'write_result', request: m.request, ok: false, message: 'not a file picked for saving' });
  if (m.offset === 0) s.parts = [];
  s.parts.push(payload.slice());
  if (m.last) {
    download(URL.createObjectURL(new Blob(s.parts)), s.name);
    saves.delete(m.uri);
  }
  send({ t: 'write_result', request: m.request, ok: true });
}

// What the browser knows of Android's state, for the Settings and NetworkMonitor portals.
const darkQuery = matchMedia('(prefers-color-scheme: dark)');
const motionQuery = matchMedia('(prefers-reduced-motion: reduce)');
const contrastQuery = matchMedia('(prefers-contrast: more)');
let battery = null;

function sendSystem() {
  if (!ready) return;
  const mem = (navigator.deviceMemory || 0) * (1 << 30);
  send({
    t: 'system', dark: darkQuery.matches, high_contrast: contrastQuery.matches, reduced_motion: motionQuery.matches,
    font_scale: 1, power_saver: false,
    network: { available: navigator.onLine, metered: !!(navigator.connection && navigator.connection.saveData), connectivity: navigator.onLine ? 4 : 1 },
    memory: { total: mem, available: mem, low: false },
    battery: battery ? { level: Math.round(battery.level * 100), charging: battery.charging } : undefined,
    locale: navigator.language || 'en-US',
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC',
  });
}

for (const q of [darkQuery, motionQuery, contrastQuery]) q.addEventListener('change', sendSystem);
window.addEventListener('online', sendSystem);
window.addEventListener('offline', sendSystem);
if (navigator.getBattery) {
  navigator.getBattery().then((b) => {
    battery = b;
    b.addEventListener('levelchange', sendSystem);
    b.addEventListener('chargingchange', sendSystem);
    sendSystem();
  }).catch(() => {});
}

// ------------------------------------------------------------------ start

if (!window.isSecureContext || !crypto.subtle) {
  showPanel(`<p class="err">Open this page as <code>http://127.0.0.1:${esc(location.port)}/</code>: the browser only allows what Lindroids needs there.</p>`);
} else {
  connect();
}
