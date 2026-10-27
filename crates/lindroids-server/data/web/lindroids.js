// Lindroids in the browser: shows the Linux windows of the Lindroids server that served this page,
// and sends back touch, mouse, pen, keys and text.
//
// The page speaks the server's link protocol (crates/lindroids-proto) over the WebSocket at /link:
// each message from the server is one WebSocket message (a JSON line, then its payload, if any).
// Both ends prove themselves with ECDSA P-256 keys (docs/DISPLAY.md, "The handshake"): this page
// made its key pair when it paired (the private key cannot be read out, only used) and keeps it
// in IndexedDB with the relay's public key, pinned then. It pairs with the ticket `lindroids run`
// puts in the URL's fragment (#t=...), or with the code `lindroids pair` shows.
'use strict';

const BUILD = '@BUILD@';
const PROTOCOL_VERSION = 5;
// What pages kept before keys (protocol 4): gone, it no longer proves anything.
try { localStorage.removeItem('lindroids.secret'); } catch (e) { /* storage off */ }

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

// The ticket `lindroids run` put in the URL, for pairing: kept in memory only, and out of the
// address bar, history and bookmarks at once.
let ticket = null;
function takeTicketFromUrl() {
  const m = /[#&]t=([^&]+)/.exec(location.hash);
  if (!m) return false;
  ticket = decodeURIComponent(m[1]);
  history.replaceState(null, '', location.pathname + location.search);
  return true;
}
takeTicketFromUrl();

// `lindroids run` opens the page again to bring it to the front; the browser reuses this tab and
// only the fragment changes. If this tab had lost the link (to another tab, or a server that
// restarted), take it back now: the user asked for it.
window.addEventListener('hashchange', () => {
  if (!takeTicketFromUrl()) return;
  if (!ws || !ready) connect();
});

// What this page keeps: its key pair, its display id, the relay's key (IndexedDB: a CryptoKey
// is stored as itself, so a non-extractable private key stays one).
function store(mode, f) {
  return new Promise((resolve, reject) => {
    const open = indexedDB.open('lindroids', 1);
    open.onupgradeneeded = () => open.result.createObjectStore('kv');
    open.onerror = () => reject(open.error);
    open.onsuccess = () => {
      const tx = open.result.transaction('kv', mode);
      const req = f(tx.objectStore('kv'));
      tx.oncomplete = () => { open.result.close(); resolve(req && req.result); };
      tx.onerror = () => { open.result.close(); reject(tx.error); };
    };
  });
}
const kvGet = (k) => store('readonly', (s) => s.get(k));
const kvSet = (k, v) => store('readwrite', (s) => s.put(v, k));
const kvDel = (k) => store('readwrite', (s) => s.delete(k));

function b64dec(s) {
  const b = atob(s.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(b, (c) => c.charCodeAt(0));
}

// What each side signs: a label, this page's origin and both nonces (auth.rs in lindroids-proto).
function transcript(label, a, b) {
  return enc.encode([label, location.origin, a, b].join('\0'));
}
const ECDSA = { name: 'ECDSA', namedCurve: 'P-256' };
const SIGN = { name: 'ECDSA', hash: 'SHA-256' };
let pairing = null; // { keys, relayKey } until the relay says Paired

function b64url(buf) {
  let s = '';
  for (const b of new Uint8Array(buf)) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}


function randomNonce() {
  const a = new Uint8Array(24);
  crypto.getRandomValues(a);
  return b64url(a);
}

// ------------------------------------------------------------------ the link

// A page nobody can see does not take the link: browsers bring back old tabs in the background
// (Firefox restores every tab when it starts), and the newest page to link wins. It links once shown.
let linkWhenShown = false;
function connect() {
  if (document.hidden) {
    if (!linkWhenShown) {
      linkWhenShown = true;
      document.addEventListener('visibilitychange', function shown() {
        if (document.hidden) return;
        document.removeEventListener('visibilitychange', shown);
        linkWhenShown = false;
        connect();
      });
    }
    return;
  }
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
      page: location.pathname + location.search,
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
    for (const s of [...mics.keys()]) recordClose(s);
    if (replaced) return;
    // A program's own window: the program cannot outlive the server (its display went with it;
    // a run from a home screen icon ends with Termux's session, server and all). That is its end.
    if (OWN_WINDOW && RUN && was) {
      if (ended === null) ended = program ? program.name : 'The program';
      return showEnded();
    }
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
      // The relay first: it signed our nonce with its key. Once paired, only the key we pinned.
      const pinned = await kvGet('relay_key');
      const relayKey = await crypto.subtle.importKey('raw', b64dec(m.relay_key), ECDSA, false, ['verify']);
      const genuine = await crypto.subtle.verify(SIGN, relayKey, b64dec(m.proof), transcript('lindroids/relay', myNonce, m.nonce));
      const code = pendingCode;
      pendingCode = null;
      // Another key than the one pinned: not the relay this browser paired with (something took
      // its port), unless the user says otherwise with the pairing code (Lindroids reinstalled).
      if (!genuine || (pinned && pinned !== m.relay_key && !code)) {
        replaced = true;
        ws.close();
        return askCode(null, 'This is not the Lindroids this browser paired with: something else answers on its port. ' +
          'If you reinstalled Lindroids, pair again with the code from `lindroids pair`.');
      }
      const signed = transcript('lindroids/display', m.nonce, myNonce);
      const keys = await kvGet('keys'), display = await kvGet('display');
      // The ticket goes along every time: a run's window attaches with it.
      if (pinned === m.relay_key && keys && display) {
        return send({ t: 'auth', display, proof: b64url(await crypto.subtle.sign(SIGN, keys.privateKey, signed)), ...(ticket ? { ticket } : {}) });
      }
      if (!ticket && !code) return askCode(null, '');
      // Pairing: a new key pair whose private half never leaves WebCrypto.
      const fresh = await crypto.subtle.generateKey(ECDSA, false, ['sign', 'verify']);
      pairing = { keys: fresh, relayKey: m.relay_key };
      return send({
        t: 'auth',
        public_key: b64url(await crypto.subtle.exportKey('raw', fresh.publicKey)),
        proof: b64url(await crypto.subtle.sign(SIGN, fresh.privateKey, signed)),
        ...(code ? { code } : {}),
        ...(ticket ? { ticket } : {}),
        name: browserName(),
      });
    }
    case 'auth_failed':
      if (m.reason === 'this window was not opened by lindroids run') {
        replaced = true;
        return showPanel(`<p>This window was not opened by <code>lindroids run</code>.</p>
          <p>Start the program from Termux, or from its icon, to see it.</p>`);
      }
      // What we kept no longer works (the relay forgot this display): pair again.
      if (/not paired with this Lindroids/.test(m.reason)) await kvDel('display');
      if (!m.pairing_open) return showPanel(`<p class="err">Pairing is closed after too many wrong codes.</p>
        <p>Run <code>lindroids stop</code>, then <code>lindroids run &lt;program&gt;</code> again in Termux.</p>`);
      replaced = true; // reconnect by hand, with the code (or at once, with a ticket)
      if (ticket && m.reason !== 'not paired') { replaced = false; ws.close(); return; }
      return askCode(null, m.reason);
    case 'paired':
      if (pairing) {
        await kvSet('keys', pairing.keys);
        await kvSet('relay_key', pairing.relayKey);
        await kvSet('display', m.display);
        pairing = null;
      }
      return;
    case 'hello':
      // A page from before an update (a window raised rather than reloaded): load the new one.
      if (m.build && m.build !== BUILD && !BUILD.startsWith('@')) return location.reload();
      if (m.stats) enableStats();
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
    case 'run':
      program = { name: m.name, icon: m.icon_png_b64 || null };
      showProgram();
      return hidePanelIfWindows();
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
    case 'record_open': return recordOpen(m);
    case 'record_close': return recordClose(m.stream);
    case 'open_uri': return openUri(m);
    case 'open_file': return openFile(m, payload);
    case 'notify': return notify(m);
    case 'close_notification': return closeToast('n:' + m.id);
    case 'wake_lock': return wakeLock(m.on);
    case 'run_ended':
      if (!OWN_WINDOW || windows.size) return;
      if (ended === null) ended = program ? program.name : 'The program';
      if (m.output && m.output.length) {
        endedNote = everShown ? `It stopped with an error (exit code ${m.code}):` : `It could not start (exit code ${m.code}):`;
        endedOutput = m.output;
      } else if (!everShown) {
        endedNote = `It ended before showing a window (exit code ${m.code}); its messages are in Termux.`;
      }
      return showEnded();
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

// What this display is called in `lindroids displays`.
function browserName() {
  const ua = navigator.userAgent;
  const b = /Firefox\/[\d.]+/.exec(ua) || /(Edg|SamsungBrowser|Chrome)\/[\d.]+/.exec(ua);
  const os = /Android [\d.]+/.exec(ua) || /\(([^;)]+)/.exec(ua);
  return [b && b[0], os && (os[1] || os[0])].filter(Boolean).join(', ') || 'a browser';
}

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
}

// Lindroids' icon, kept: the panel still shows it when the server is gone.
let ownIcon = '/icon.svg';
fetch('/icon.svg').then((r) => (r.ok ? r.text() : Promise.reject())).then((t) => { ownIcon = 'data:image/svg+xml,' + encodeURIComponent(t); }).catch(() => {});

function showPanel(html, after, icon) {
  panel.hidden = false;
  panel.innerHTML = `<div class="box"><img src="${icon || ownIcon}" alt="">${html}</div>`;
  if (after) after();
}

// In a window of its own (`?window`), the program's last window closing ends the window too: it
// closes itself where the browser lets a page do that, and otherwise says so (Firefox's Custom
// Tabs ignore window.close(), and Back only hides them: their ✕ closes them). Not at once: a
// splash may give way to the window.
const OWN_WINDOW = new URLSearchParams(location.search).has('window');
const RUN = new URLSearchParams(location.search).get('run');
let ended = null, endedIcon = null, endTimer = null, endedNote = '', endedOutput = [], everShown = false;
let program = null; // { name, icon } of this window's run, from the server

// The window as the browser shows it outside the page (title, icon; Chrome puts them on its task
// in recents): the program's, not Lindroids'.
function showProgram() {
  if (!program) return;
  if (active === null) document.title = program.name;
  if (!program.icon) return;
  for (const l of document.querySelectorAll('link[rel~="icon"]')) l.remove();
  const l = document.createElement('link');
  l.rel = 'icon';
  l.type = 'image/png';
  l.href = `data:image/png;base64,${program.icon}`;
  document.head.appendChild(l);
}

// One program in a window of its own: no tab strip (see index.html, body.solo).
function updateChrome() {
  document.body.classList.toggle('solo', OWN_WINDOW && windows.size <= 1);
}
let toolsTimer = null;
function showTools(on) {
  clearTimeout(toolsTimer);
  document.body.classList.toggle('tools', on);
  if (on) toolsTimer = setTimeout(() => showTools(false), 5000);
}
$('handle').onclick = (e) => { e.stopPropagation(); showTools(!document.body.classList.contains('tools')); };
$('tools').addEventListener('click', () => showTools(true));

function showEnded() {
  const icon = endedIcon || (program && program.icon);
  const out = endedOutput.length ? `<pre class="output">${esc(endedOutput.join('\n'))}</pre>` : '';
  showPanel(`<p>${esc(program ? program.name : ended)} ${endedOutput.length ? 'stopped' : 'closed'}.</p>${endedNote ? `<p>${esc(endedNote)}</p>` : ''}${out}<p>Close this window with the ✕ at the top.</p>`, null,
    icon && `data:image/png;base64,${icon}`);
  // Out of full screen, so the browser's own close button shows where the page cannot close.
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  endTimer = setTimeout(() => { if (!windows.size) window.close(); }, 1500);
}

// Full screen that arrives after the end (the tap that closed the program asked for it too).
document.addEventListener('fullscreenchange', () => {
  if (document.fullscreenElement && ended !== null && !windows.size) document.exitFullscreen().catch(() => {});
});

function hidePanelIfWindows() {
  if (!ready) return;
  clearTimeout(endTimer);
  if (windows.size) { ended = null; panel.hidden = true; return; }
  if (ended !== null) return showEnded();
  if (program) {
    return showPanel(`<p>Starting ${esc(program.name)}…</p>`, null, program.icon && `data:image/png;base64,${program.icon}`);
  }
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
  everShown = true;
  updateChrome();
  attachInput(w);
  resizeWindow(w);
  activate(m.win);
}

function removeWindow(win) {
  const w = windows.get(win);
  if (!w) return;
  windows.delete(win);
  updateChrome();
  if (OWN_WINDOW && !windows.size) { ended = w.title || 'The program'; endedIcon = w.icon || null; }
  w.canvas.remove();
  w.tab.remove();
  const i = order.indexOf(win);
  if (i >= 0) order.splice(i, 1);
  if (active === win) {
    active = null;
    const next = order[order.length - 1];
    if (next !== undefined) activate(next);
    else { document.title = program ? program.name : 'Lindroids'; hideKeyboard(); }
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

// The page never scrolls, but Firefox leaves it scrolled by its toolbar's height when full screen
// ends while the window is in the background: the program's window then shows shifted up. Back to
// the top whenever the size or the visibility changes.
function unscroll() {
  if (STATS) {
    const r = stage.getBoundingClientRect(), w = windows.get(active), vv = window.visualViewport;
    plog(`layout: inner ${innerWidth}x${innerHeight} scroll ${scrollX},${scrollY} stage ${r.top},${r.height} vv ${vv ? `${vv.offsetTop},${vv.height}` : '-'} canvas ${w ? `${w.canvas.width}x${w.canvas.height} at ${w.canvas.getBoundingClientRect().top},${w.canvas.getBoundingClientRect().height}` : '-'} fs ${!!document.fullscreenElement}`);
  }
  if (window.scrollX || window.scrollY) window.scrollTo(0, 0);
}
for (const ev of ['resize', 'fullscreenchange', 'visibilitychange', 'pageshow']) addEventListener(ev, () => requestAnimationFrame(unscroll));
if (window.visualViewport) visualViewport.addEventListener('resize', () => requestAnimationFrame(unscroll));

// Sizes sent while the page was hidden (full screen ending behind another app) may not take, and
// the size settles only after the browser's bars do: on coming back, and when full screen
// changes, every window's size again.
function resizeAgain() {
  if (document.hidden) return;
  for (const t of [0, 400]) setTimeout(() => { for (const w of windows.values()) { w.size = [0, 0]; resizeWindow(w); ackLater(w); } }, t);
}
document.addEventListener('visibilitychange', resizeAgain);
document.addEventListener('fullscreenchange', resizeAgain);

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

// The server runs with LINDROIDS_STATS=1 (its hello says so): frames, data and drawing time each
// second, in a corner, and the page's layout in the server's log. Not in the URL: a window's URL
// must stay the one Android raises it by.
let STATS = null;
function enableStats() {
  if (STATS) return;
  STATS = { frames: 0, bytes: 0, pixels: 0, ms: 0, el: null };
  STATS.el = document.createElement('div');
  STATS.el.style.cssText = 'position:fixed;left:4px;bottom:4px;z-index:9;font:12px monospace;color:#0f0;background:#000a;padding:2px 6px;pointer-events:none';
  document.body.appendChild(STATS.el);
  setInterval(() => {
    STATS.el.textContent = `${STATS.frames} fps  ${(STATS.bytes / 1024).toFixed(0)} KiB/s  ${(STATS.pixels / 1e6).toFixed(2)} Mpx/s  draw ${STATS.frames ? (STATS.ms / STATS.frames).toFixed(1) : 0} ms`;
    STATS.frames = STATS.bytes = STATS.pixels = STATS.ms = 0;
  }, 1000);
}

function tiles(m, payload) {
  const w = windows.get(m.win);
  if (!w) return send({ t: 'frame_done', win: m.win });
  const t = STATS && performance.now();
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
  if (STATS) { STATS.frames++; STATS.bytes += payload.length; STATS.pixels += size / 4; STATS.ms += performance.now() - t; }
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
// A window of its own (a Custom Tab opened by `lindroids run`, `?window`): the first tap takes the
// whole screen, so nothing of the browser is left. Browsers allow full screen only while handling
// a tap; the canvas cancels touches (no click follows), so ask on the tap's end. Not on the
// page's own buttons: asking for full screen uses up the tap, and they need it (Choose… opens
// Android's picker).
if (new URLSearchParams(location.search).has('window')) {
  const full = (e) => {
    // Not once the program has ended: the browser's close button is the way out then.
    if (ended !== null && !windows.size) return;
    if (e.target instanceof Element && e.target.closest('.dialog, .toast')) return;
    if (!document.fullscreenElement) document.documentElement.requestFullscreen({ navigationUI: 'hide' }).catch(() => {});
  };
  for (const ev of ['pointerup', 'touchend', 'click']) addEventListener(ev, full, { capture: true });
}
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
  for (const r of mics.values()) if (r.ctx && r.ctx.state === 'suspended') r.ctx.resume();
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

// ------------------------------------------------------------------ microphone

// A program records (a PulseAudio record stream): the page captures the microphone, the browser
// asking the first time, and sends interleaved s16le at the stream's rate and channel count,
// about 20 ms at a time. Without the microphone (refused, none) the server records silence.
const mics = new Map(); // stream -> {rate, channels, media, ctx, node, closed}
let micToastShown = false;

async function recordOpen(m) {
  recordClose(m.stream);
  const r = { stream: m.stream, rate: m.rate, channels: m.channels, closed: false };
  mics.set(m.stream, r);
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
    return micFailed(r, 'this browser gives no microphone here (getUserMedia is missing)');
  }
  try {
    r.media = await navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: { ideal: m.channels } },
    });
  } catch (e) {
    return micFailed(r, `the browser refused it (${e.name}: ${e.message})`);
  }
  if (r.closed) return micStop(r);
  try {
    await micCapture(r);
  } catch (e) {
    micFailed(r, `capture failed: ${e}`);
  }
}

function micFailed(r, why) {
  plog(`microphone for stream ${r.stream}: ${why}`);
  micStop(r);
  if (micToastShown || r.closed) return;
  micToastShown = true;
  toast({ title: 'No microphone', body: 'The program records silence. To record, allow the microphone for this page in the browser\'s site settings, then start recording again.' }, 8000);
}

// Capture into r.ctx: at the stream's rate if the browser resamples the microphone itself (Chrome
// does), else at the context's own rate, resampled here.
async function micCapture(r) {
  const AC = window.AudioContext || window.webkitAudioContext;
  if (!AC) throw new Error('no Web Audio');
  let src = null;
  try {
    r.ctx = new AC({ sampleRate: r.rate, latencyHint: 'interactive' });
    src = r.ctx.createMediaStreamSource(r.media);
  } catch (e) {
    if (r.ctx) r.ctx.close().catch(() => {});
    r.ctx = new AC({ latencyHint: 'interactive' });
    src = r.ctx.createMediaStreamSource(r.media);
  }
  const ctx = r.ctx;
  const resample = micResampler(ctx.sampleRate, r.rate, r.channels);
  const onSamples = (chans) => {
    if (r.closed) return;
    const pcm = resample(chans);
    if (pcm.length) send({ t: 'record_data', stream: r.stream, len: pcm.byteLength }, new Uint8Array(pcm.buffer));
  };
  if (ctx.audioWorklet && window.AudioWorkletNode) {
    await ctx.audioWorklet.addModule('/mic-worklet.js');
    if (r.closed) return micStop(r);
    r.node = new AudioWorkletNode(ctx, 'lindroids-mic', { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1] });
    r.node.port.onmessage = (e) => onSamples(e.data);
  } else {
    // Older browsers: the main thread gets the samples.
    const size = ctx.sampleRate > 24000 ? 1024 : 512;
    r.node = ctx.createScriptProcessor(size, 2, 1);
    r.node.onaudioprocess = (e) => {
      const b = e.inputBuffer;
      const chans = [];
      for (let c = 0; c < b.numberOfChannels; c++) chans.push(b.getChannelData(c).slice());
      onSamples(chans);
    };
  }
  src.connect(r.node);
  // Its output is silence: connected only so the browser keeps running it.
  r.node.connect(ctx.destination);
  if (ctx.state !== 'running') {
    ctx.resume().catch(() => {});
    // The browser starts sound only after a tap on the page: say so if it holds this back.
    setTimeout(() => {
      if (!r.closed && ctx.state !== 'running') {
        plog(`microphone for stream ${r.stream}: waiting for a tap to start (${ctx.state})`);
        toast({ title: 'Tap the window to start the microphone', body: 'The browser starts sound only after a tap.' }, 5000);
      }
    }, 500);
  }
}

// From the capture's channels at rate `from` to interleaved s16 with `channels` channels at
// `to`: linear interpolation, carried across chunks.
function micResampler(from, to, channels) {
  const step = from / to;
  let pos = 0;
  const last = new Float32Array(channels);
  return (chans) => {
    if (!chans.length || !chans[0].length) return new Int16Array(0);
    const n = chans[0].length;
    const ch = [];
    for (let c = 0; c < channels; c++) {
      if (chans.length >= channels) ch.push(chans[c]);
      else if (channels === 2) ch.push(chans[0]);
    }
    if (channels === 1 && chans.length > 1) {
      const mix = new Float32Array(n);
      for (let i = 0; i < n; i++) mix[i] = (chans[0][i] + chans[1][i]) / 2;
      ch[0] = mix;
    }
    const s16 = (v) => Math.max(-32768, Math.min(32767, Math.round(v * 32768)));
    if (step === 1) {
      const out = new Int16Array(n * channels);
      for (let i = 0; i < n; i++) for (let c = 0; c < channels; c++) out[i * channels + c] = s16(ch[c][i]);
      return out;
    }
    const frames = [];
    while (pos < n - 1) {
      const i = Math.floor(pos), f = pos - i;
      for (let c = 0; c < channels; c++) {
        const a = i < 0 ? last[c] : ch[c][i];
        frames.push(s16(a + (ch[c][i + 1] - a) * f));
      }
      pos += step;
    }
    pos -= n;
    for (let c = 0; c < channels; c++) last[c] = ch[c][n - 1];
    return Int16Array.from(frames);
  };
}

function recordClose(stream) {
  const r = mics.get(stream);
  if (!r) return;
  r.closed = true;
  mics.delete(stream);
  micStop(r);
}

function micStop(r) {
  if (r.node) {
    try { if (r.node.port) r.node.port.postMessage('stop'); r.node.disconnect(); } catch (e) { /* gone */ }
    r.node = null;
  }
  if (r.media) { for (const t of r.media.getTracks()) t.stop(); r.media = null; }
  if (r.ctx) { r.ctx.close().catch(() => {}); r.ctx = null; }
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

// Into the server's log (`server.log` in the runtime dir): what a phone's browser keeps to itself.
function plog(msg) { send({ t: 'log', msg: String(msg) }); }
addEventListener('error', (e) => plog(`error: ${e.message} (${e.filename}:${e.lineno})`));
addEventListener('unhandledrejection', (e) => plog(`unhandled: ${e.reason}`));

// A program keeps the screen on (a video, a presentation): Android's screen wake lock, while
// this window is shown (the browser lets it go when it is hidden; it is taken again on return).
let wantWake = false, wake = null;
async function wakeLock(on) {
  wantWake = on;
  if (!navigator.wakeLock) return plog('no screen wake lock in this browser');
  if (!on) { if (wake) { const w = wake; wake = null; await w.release().catch(() => {}); } return; }
  if (wake || document.hidden) return;
  try {
    wake = await navigator.wakeLock.request('screen');
    wake.addEventListener('release', () => { wake = null; });
    plog('screen wake lock held');
  } catch (e) { plog(`screen wake lock: ${e}`); }
}
document.addEventListener('visibilitychange', () => { if (!document.hidden && wantWake) wakeLock(true); });

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

// File System Access (Chromium 132+ on Android): picked documents can be written back, a save goes
// where the user says, and folders can be picked. Without it (Firefox) a pick is a copy and a
// save is a download.
const fsa = typeof window.showOpenFilePicker === 'function';
const handles = new Map(); // uri -> the picked file's handle, or the picked folder's
const writes = new Map(); // uri -> the write going on (a promise of its stream; null when done)

// The last tap or click on the page, for what a browser allows only right after one.
let lastTap = -1e9;
addEventListener('pointerup', () => { lastTap = performance.now(); }, { capture: true });

function pickFiles(p) {
  const cancel = () => send({ t: 'files_picked', request: p.request, files: [] });
  if (fsa) return pickHandles(p, cancel);
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
  const key = 'pick:' + p.request;
  input.onchange = () => {
    closeToast(key);
    const files = [...input.files];
    picked.set(p.request, files);
    send({ t: 'files_picked', request: p.request, files: files.map((f, i) => ({ uri: `pick:${p.request}:${i}`, name: f.name, size: f.size, mime: f.type || undefined })) });
  };
  input.oncancel = () => { closeToast(key); cancel(); };
  const title = p.title || 'A Linux program wants a file';
  // The file chooser needs a tap on this page. The tap that asked for it (Open in the program) is
  // still fresh for a few seconds: straight to Android's picker then, with a note in the corner
  // in case the browser did not show it (it goes when a file is picked or the picker cancelled).
  if (activeGesture() || performance.now() - lastTap < 4000) {
    input.click();
    toast({ key, title, actions: [['choose', 'Choose…'], ['cancel', 'Cancel']], onAction: (k) => (k === 'choose' ? input.click() : cancel()) }, 0);
    return;
  }
  dialog(`<p>${esc(title)}</p>`, [['Choose…', () => input.click()], ['Cancel', cancel, true]]);
}

// Android's picker through File System Access. Like the file chooser it needs a tap: the one that
// asked (Open in the program) if it is fresh, else a tap on a dialog.
function pickHandles(p, cancel) {
  const title = p.title || (p.directory ? 'A Linux program wants a folder' : p.save ? 'Save' : 'A Linux program wants a file');
  const ask = () => dialog(`<p>${esc(title)}</p>`, [['Choose…', go], ['Cancel', cancel, true]]);
  function go() {
    showPicker(p).then((files) => send({ t: 'files_picked', request: p.request, files }), (e) => {
      if (e.name === 'SecurityError' || e.name === 'NotAllowedError') return ask();
      if (e.name !== 'AbortError') plog(`picker: ${e}`);
      cancel();
    });
  }
  if (activeGesture() || performance.now() - lastTap < 4000) go();
  else ask();
}

function pickTypes(p) {
  const mimes = (p.mime_types || []).filter((m) => /^[\w.+-]+\/[\w.+*-]+$/.test(m));
  return mimes.length ? [{ description: p.title || 'Files', accept: Object.fromEntries(mimes.map((m) => [m, []])) }] : undefined;
}

async function showPicker(p) {
  const types = pickTypes(p);
  if (p.save) {
    const h = await showSaveFilePicker({ suggestedName: p.name || 'file', types });
    const uri = 'save:' + p.request;
    handles.set(uri, h);
    return [{ uri, name: h.name, size: 0, writable: true }];
  }
  if (p.directory) {
    const d = await showDirectoryPicker({ mode: 'readwrite' });
    const uri = 'dir:' + p.request;
    handles.set(uri, d);
    const files = [{ uri, name: d.name, size: 0, mime: 'inode/directory', rel: '', writable: true }];
    const blobs = [null];
    await listFolder(d, '', uri, files, blobs);
    picked.set(p.request, blobs);
    return files;
  }
  const hs = await showOpenFilePicker({ multiple: !!p.multiple, types });
  const blobs = await Promise.all(hs.map((h) => h.getFile()));
  picked.set(p.request, blobs);
  return hs.map((h, i) => {
    const uri = `pick:${p.request}:${i}`;
    handles.set(uri, h);
    return { uri, name: h.name, size: blobs[i].size, mime: blobs[i].type || undefined, writable: true };
  });
}

// Everything in a picked folder, folders first in each, as files_picked entries (and the files to
// fetch, at the same index).
async function listFolder(d, prefix, uri, files, blobs) {
  for await (const [name, h] of d.entries()) {
    if (files.length > 20000) throw new Error(`${d.name}: too many files`);
    const rel = prefix + name;
    if (h.kind === 'directory') {
      files.push({ uri: `${uri}/${rel}`, name, size: 0, mime: 'inode/directory', rel, writable: true });
      blobs.push(null);
      await listFolder(h, rel + '/', uri, files, blobs);
    } else {
      const f = await h.getFile();
      files.push({ uri: `${uri}/${rel}`, name, size: f.size, mime: f.type || undefined, rel, writable: true });
      blobs.push(f);
    }
  }
}

// A picked file's handle; in a picked folder ("dir:<request>/<path>"), made if it is new.
async function fileHandle(uri) {
  const h = handles.get(uri);
  if (h) return h;
  const m = /^(dir:\d+)\/(.+)$/.exec(uri);
  let d = m && handles.get(m[1]);
  if (!d) throw new Error('not a picked file');
  const parts = m[2].split('/');
  for (const part of parts.slice(0, -1)) d = await d.getDirectoryHandle(part, { create: true });
  return d.getFileHandle(parts[parts.length - 1], { create: true });
}

// Into the picked document itself. The first write into a file picked to open asks "Save
// changes?" once; no tap is needed.
function writeHandle(m, payload) {
  const data = payload.slice();
  let w = (writes.get(m.uri) || Promise.resolve(null)).catch(() => null);
  if (m.offset === 0) {
    w = w.then(async (old) => {
      if (old) await old.abort().catch(() => {});
      return (await fileHandle(m.uri)).createWritable();
    });
  }
  w = w.then(async (s) => {
    if (!s) throw new Error('a write without its start');
    await s.write(data);
    return s;
  });
  if (m.last) {
    w = w.then(async (s) => { await s.close(); return null; });
    w.then(() => send({ t: 'write_result', request: m.request, ok: true }),
      (e) => send({ t: 'write_result', request: m.request, ok: false, message: String(e) }));
  }
  writes.set(m.uri, w);
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
  if (handles.has(m.uri) || /^dir:\d+\//.test(m.uri)) return writeHandle(m, payload);
  const s = saves.get(m.uri);
  if (!s) return send({ t: 'write_result', request: m.request, ok: false, message: 'not a file picked for saving' });
  if (m.offset === 0) s.parts = [];
  s.parts.push(payload.slice());
  if (m.last) {
    // Kept: the program saves to the same file again (Ctrl+S), and each save is downloaded anew.
    const url = URL.createObjectURL(new Blob(s.parts));
    download(url, s.name);
    s.parts = [];
    setTimeout(() => URL.revokeObjectURL(url), 60000);
  }
  send({ t: 'write_result', request: m.request, ok: true });
}

// What the browser knows of Android's state, for the Settings and NetworkMonitor portals.
const darkQuery = matchMedia('(prefers-color-scheme: dark)');
const motionQuery = matchMedia('(prefers-reduced-motion: reduce)');
const contrastQuery = matchMedia('(prefers-contrast: more)');
let battery = null;

// Android's accent colour (Material You), as the browser shows it: the CSS system colour
// AccentColor, sRGB 0..1. libadwaita 1.6+ takes it through the portal (accent-color).
function accentColor() {
  if (!(window.CSS && CSS.supports('color', 'AccentColor'))) return undefined;
  const d = document.createElement('div');
  d.style.color = 'AccentColor';
  document.body.appendChild(d);
  const m = /rgba?\((\d+),\s*(\d+),\s*(\d+)/.exec(getComputedStyle(d).color);
  d.remove();
  return m ? [m[1] / 255, m[2] / 255, m[3] / 255] : undefined;
}

function sendSystem() {
  if (!ready) return;
  const mem = (navigator.deviceMemory || 0) * (1 << 30);
  send({
    t: 'system', dark: darkQuery.matches, accent: accentColor(), high_contrast: contrastQuery.matches, reduced_motion: motionQuery.matches,
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

// The page is going away for good (its window closed, or a reload): say so, so the server does
// not wait a minute for a browser that might come back. A page put in the back/forward cache
// (persisted) is not going away.
addEventListener('pagehide', e => { if (!e.persisted && ready) send({ t: 'closing' }); });

// ------------------------------------------------------------------ start

if (!window.isSecureContext || !crypto.subtle) {
  showPanel(`<p class="err">Open this page as <code>http://127.0.0.1:${esc(location.port)}/</code>: the browser only allows what Lindroids needs there.</p>`);
} else {
  connect();
}
