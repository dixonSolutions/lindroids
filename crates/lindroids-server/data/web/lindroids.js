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
const PICKER_TOKEN = '@PICKER_TOKEN@';
const PROTOCOL_VERSION = 6;
// What pages kept before keys (protocol 4): gone, it no longer proves anything.
try { localStorage.removeItem('lindroids.secret'); } catch (e) { /* storage off */ }

const $ = (id) => document.getElementById(id);
const tabsEl = $('tabs'), stage = $('stage'), panel = $('panel'), ime = $('ime'), toastsEl = $('toasts');
const enc = new TextEncoder(), dec = new TextDecoder();
// The Lindroids window app (BROWSER_AGNOSTIC_APK.md) shows this page in Android's own WebView and
// exposes a small bridge for what WebView lacks: Android's file pickers and downloads, the
// clipboard both ways, sharing, closing the task. In a browser there is no bridge.
const NATIVE = window.lindroids || null;

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

// The ticket `lindroids run` put in the URL, for pairing and for attaching to its run: out of the
// address bar, history and bookmarks at once. It stays with this window's history entry
// (history.state, which no other page can read), so the page the browser loads again after it
// dropped this one in the background, or a reload, still attaches to its run.
let ticket = null, ticketRestored = false;
function takeTicketFromUrl() {
  const m = /[#&]t=([^&]+)/.exec(location.hash);
  if (!m) return false;
  ticket = decodeURIComponent(m[1]);
  history.replaceState({ t: ticket }, '', location.pathname + location.search);
  return true;
}
if (!takeTicketFromUrl() && history.state && typeof history.state.t === 'string') {
  ticket = history.state.t;
  ticketRestored = true;
}

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
  // Linking again to the run this window showed is fine unseen: it is this window's own.
  if (document.hidden && !relinking) {
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
  ws.onclose = (ev) => {
    trace(`link closed (code ${ev.code}, ${document.hidden ? 'hidden' : 'shown'})`);
    ws = null;
    const was = ready;
    ready = false;
    for (const w of [...windows.keys()]) removeWindow(w);
    for (const s of [...mics.keys()]) recordClose(s);
    if (replaced) return;
    // A program's own window. The program ended (the server said so), or cannot outlive a server
    // that is gone (its display went with it; a run from a home screen icon ends with Termux's
    // session, server and all): that is its end. A link the browser dropped while the program
    // runs is not (a phone's browser in the background): link again at once, seen or not; the
    // server keeps the run for a while (docs/DISPLAY.md, "The grace period").
    if (OWN_WINDOW && RUN && (was || relinking)) {
      if (!runOver && (was || relinking++ < 2)) {
        if (was) {
          relinking = 1;
          lostLink = `code ${ev.code}, ${document.hidden ? 'hidden' : 'shown'}`;
        }
        ended = null;
        if (program) showPanel(`<p>Connecting to ${esc(program.name)} again…</p>`, null, program.icon && `data:image/png;base64,${program.icon}`);
        retry = setTimeout(connect, was ? 0 : 1000);
        return;
      }
      relinking = 0;
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
        // Its ticket worked before (on this page, or the one before a reload): its run is over
        // (the server forgets a run's ticket with it).
        if (OWN_WINDOW && RUN && (relinking || everLinked || ticketRestored)) {
          relinking = 0;
          if (ended === null) ended = program ? program.name : (history.state && history.state.name) || 'The program';
          return showEnded();
        }
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
      everLinked = true;
      if (document.hidden) send({ t: 'shown', shown: false });
      for (const l of traced.splice(0)) plog(`trace: ${l} (while unlinked)`);
      if (relinking) {
        plog(`linked again after the browser dropped the link (${lostLink})`);
        relinking = 0;
      }
      if (APP && !OWN_WINDOW) return appPage();
      hidePanelIfWindows();
      sendSystem();
      return;
    case 'navigate': {
      // To the window of the program this page asked for: only within this program's pages.
      const to = new URL(m.url, location.href);
      if (!APP || to.origin !== location.origin || !to.pathname.startsWith(`/app/${APP}/`)) return;
      replaced = true;
      return location.replace(to.href);
    }
    case 'bye':
      replaced = true;
      // A tab the browser opened for `lindroids run` that a newer one replaced: close it, where
      // the browser lets a page do that (one history entry), so tabs do not pile up.
      if (history.length <= 1) setTimeout(() => window.close(), 300);
      return showPanel(`<p>${esc(m.reason)}.</p><p><button id="again">Show the windows here</button></p>`, () => $('again').onclick = connect);
    case 'run':
      program = { name: m.name, icon: m.icon_png_b64 || null };
      // For its end screen, should the page be loaded again after the program ended.
      if (ticket) history.replaceState({ t: ticket, name: m.name }, '');
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
      runOver = true;
      trace(`the program ended (exit code ${m.code})`);
      if (!OWN_WINDOW) return;
      if (ended === null) ended = program ? program.name : 'The program';
      if (windows.size) return; // its windows go next, then the end shows
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

// In a window of its own (`?window`), the program ending ends the window too: it closes itself
// where the browser lets a page do that, and otherwise says so (Firefox's Custom Tabs ignore
// window.close(), and Back only hides them: their ✕ closes them).
const OWN_WINDOW = new URLSearchParams(location.search).has('window');
const RUN = new URLSearchParams(location.search).get('run');
// An exported program's own page (`lindroids export --webapk`, /app/<id>/), which the browser
// installs as an app of its own: `?install` is where that is done; with no run, it is what the
// app opens with, which asks for the program (`launch`) and goes to its window (`navigate`).
const APP = (/^\/app\/([A-Za-z0-9._-]+)\/$/.exec(location.pathname) || [])[1] || null;
const INSTALL = !!APP && new URLSearchParams(location.search).has('install');
const FRESH = INSTALL && new URLSearchParams(location.search).has('fresh');
let ended = null, endedIcon = null, endTimer = null, endedNote = '', endedOutput = [], everShown = false;
let program = null; // { name, icon } of this window's run, from the server
// The server said the program ended; this page linked to its run before; tries at linking again
// after a lost link (0: none under way), and how that link was lost (for the server's log).
let runOver = false, everLinked = false, relinking = 0, lostLink = '';

// ------------------------------------------------------------------ an exported program's app

const appIcon = () => `/app/${APP}/icon.png`;
let installPrompt = null, installed = false;
window.addEventListener('beforeinstallprompt', (e) => {
  e.preventDefault();
  installPrompt = e;
  if (INSTALL && ready) showInstall();
});
let installReported = false;
function reportInstalled() {
  if (!INSTALL || installReported) return;
  if (!ws || ws.readyState !== WebSocket.OPEN) { setTimeout(reportInstalled, 500); return; }
  installReported = true;
  send({ t: 'installed', id: APP });
}
function installedAsWebApp() {
  if (!navigator.getInstalledRelatedApps) return Promise.resolve(false);
  return navigator.getInstalledRelatedApps()
    .then((apps) => apps.some((a) => a.platform === 'webapp'))
    .catch(() => false);
}
window.addEventListener('appinstalled', () => {
  // Chrome fires this for a home-screen shortcut too. Only a real installed web app counts.
  installedAsWebApp().then((webapp) => {
    if (webapp) {
      installed = true;
      if (INSTALL) showInstall();
      reportInstalled();
    } else if (INSTALL) {
      showPanel(`<p>Chrome added a <b>shortcut</b>, not an app of its own. This page cannot become a
        WebAPK: Chrome builds those from a public HTTPS address, and Lindroids serves this page on
        this phone only.</p><p>Use the Lindroids icon (with Termux's badge) for a window without tabs.</p>`, null, appIcon());
    }
  });
});
// Already installed (a second export, or the install finished after the export gave up): the
// installed app names this page's manifest as its own. Chrome may hide its package from queries.
if (INSTALL && !FRESH && navigator.getInstalledRelatedApps) {
  navigator.getInstalledRelatedApps().then((apps) => {
    if (apps.some((a) => a.platform === 'webapp')) {
      installed = true;
      if (ready) showInstall();
      reportInstalled();
    }
  }).catch(() => {});
}

function appPage() {
  if (INSTALL) return showInstall();
  showPanel(`<p>Starting ${esc(document.title)}…</p>`, null, appIcon());
  send({ t: 'launch', id: APP });
}

// Installing as an app of its own is the browser's to do: its install dialog (Chrome and Samsung
// Internet show it for `prompt()`), or its menu.
function showInstall() {
  const name = esc(document.title);
  if (installed) {
    return showPanel(`<p><b>${name}</b> is installed. Its icon is on the home screen and in the app list.</p>
      <p>Termux says when it has found it; this tab can be closed.</p>`, null, appIcon());
  }
  if (!installPrompt) {
    return showPanel(`<p>Install <b>${name}</b> from this browser's menu: <b>⋮</b>, then <b>Install app</b>
      (or <b>Add to home screen</b>, then <b>Install</b>).</p>
      <p class="err">A shortcut is not enough: only an app opens it as a window of its own.</p>`, null, appIcon());
  }
  showPanel(`<p>Install <b>${name}</b> as an app of its own: its icon and name on the home screen, in the
    app list and in Recents.</p><p><button id="install">Install</button></p>`, () => {
    $('install').onclick = () => {
      const p = installPrompt;
      installPrompt = null;
      p.prompt();
      p.userChoice.then((c) => { if (c.outcome !== 'accepted') { installPrompt = null; showInstall(); } }, () => showInstall());
    };
  }, appIcon());
}

// What the app shows when the server cannot be reached (Termux closed): a page of ours that
// starts the program through Termux, not the browser's error page (app-sw.js).
if (APP && 'serviceWorker' in navigator) {
  navigator.serviceWorker.register(`/app/${APP}/sw.js`, { scope: `/app/${APP}/` }).catch((e) => console.warn('lindroids: service worker:', e));
}

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
  // A URL rather than data: Chrome's Custom Tabs take the favicon for Recents only from one.
  l.href = RUN ? `/run-icon/${RUN}.png` : `data:image/png;base64,${program.icon}`;
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
  trace(`shows the end (${runOver ? 'the program ended' : 'the program is gone'})`);
  const icon = endedIcon || (program && program.icon);
  const out = endedOutput.length ? `<pre class="output">${esc(endedOutput.join('\n'))}</pre>` : '';
  const how = NATIVE ? 'The window closes with the program.' : 'Close this window with the ✕ at the top.';
  showPanel(`<p>${esc(program ? program.name : ended)} ${endedOutput.length ? 'stopped' : 'closed'}.</p>${endedNote ? `<p>${esc(endedNote)}</p>` : ''}${out}<p>${how}</p>`, null,
    icon && `data:image/png;base64,${icon}`);
  // Back closes the window again: the program is gone.
  if (backWatcher) { backWatcher.destroy(); backWatcher = null; }
  // Out of full screen, so the browser's own close button shows where the page cannot close.
  if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
  endTimer = setTimeout(() => {
    if (windows.size) return;
    trace('closes itself (window.close)');
    if (NATIVE && NATIVE.close) { NATIVE.close(); return; }
    window.close();
    // Still here a second later: the browser kept the window (Firefox's Custom Tabs always do).
    setTimeout(() => plog(`window.close() refused (history ${history.length}, ${navigator.userAgent.match(/(Firefox|Chrome)\/[\d.]+/)?.[0] || 'browser'})`), 1000);
  }, 1500);
}

// Full screen that arrives after the end (the tap that closed the program asked for it too).
document.addEventListener('fullscreenchange', () => {
  if (document.fullscreenElement && ended !== null && !windows.size) document.exitFullscreen().catch(() => {});
});

function hidePanelIfWindows() {
  if (!ready) return;
  clearTimeout(endTimer);
  if (windows.size) { panel.hidden = true; return; }
  if (ended !== null) return showEnded();
  if (program) {
    const what = everShown ? `${esc(program.name)} has no window open.` : `Starting ${esc(program.name)}…`;
    return showPanel(`<p>${what}</p>`, null, program.icon && `data:image/png;base64,${program.icon}`);
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
  // Its last window going is not its end: programs hide a window for a moment (a dialog in its
  // place, a resize, losing focus). Only the program ending is (run_ended).
  if (OWN_WINDOW && !windows.size) {
    endedIcon = w.icon || null;
    trace(`its last window went (${w.title || win})`);
  }
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

// While a composition is open, keys wait for its text: Firefox can open one for a single key
// among others (Android's injected keys) and hand over its text only when it ends, so the keys
// typed after it would overtake it ("echo" arrived as "eco…h"). Safety: they go after 2 s anyway.
let heldKeys = [], heldTimer = null;
function key(code, down) {
  const m = { t: 'key', win: active, code, down, time_ms: now() };
  if (!composing && !heldKeys.length) return send(m);
  heldKeys.push(m);
  if (!heldTimer) heldTimer = setTimeout(sendHeldKeys, 2000);
}
function sendHeldKeys() {
  clearTimeout(heldTimer);
  heldTimer = null;
  const q = heldKeys;
  heldKeys = [];
  q.forEach((m) => send(m));
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
ime.addEventListener('compositionend', () => { composing = false; imeInput(); sendHeldKeys(); });
ime.addEventListener('input', () => imeInput());

function imeInput() {
  if (active === null) return imeReset();
  const now_ = ime.value;
  let p = 0;
  while (p < imeLast.length && p < now_.length && imeLast[p] === now_[p]) p++;
  const removed = imeLast.length - p;
  const added = now_.slice(p);
  const press = (c) => { send({ t: 'key', win: active, code: c, down: true, time_ms: now() }); send({ t: 'key', win: active, code: c, down: false, time_ms: now() }); };
  for (let i = 0; i < removed; i++) press(67);
  // Enter as a key, so single-line fields activate.
  const parts = added.split('\n');
  parts.forEach((s, i) => {
    if (i > 0) press(66);
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
// Back. The browser takes a window out of full screen on Back, and closes it on the next: its
// program then ended, where the user only meant to leave it (the Back gesture is a swipe in from
// the screen's edge, made by accident too). A window of its own keeps Back from closing it
// (CloseWatcher) while its program runs: leaving is Home or the app switcher, which keep it as it
// is; closing it is the browser's ✕, or quitting the program. Browsers without CloseWatcher, and
// Chrome when the page was not touched since the last Back, close it as before.
let backWatcher = null, backHinted = 0;
function watchBack() {
  if (!OWN_WINDOW || backWatcher || typeof CloseWatcher !== 'function' || (ended !== null && !windows.size)) return;
  try {
    backWatcher = new CloseWatcher();
  } catch (e) {
    return;
  }
  backWatcher.onclose = () => {
    backWatcher = null;
    trace('Back: the window stays');
    if (performance.now() - backHinted > 30000) {
      backHinted = performance.now();
      toast({ title: program ? program.name : 'Lindroids', body: 'Home or the app switcher leaves it running. ✕ closes it.' }, 4000);
    }
    watchBack();
  };
}
watchBack();
addEventListener('pointerup', watchBack, { capture: true });
// The window app's Back: it keeps the window and says how to leave, as the page's own
// CloseWatcher does in a browser. The app itself never finishes on Back while the program runs.
if (NATIVE) addEventListener('lindroids:back', () => {
  if (ended !== null && !windows.size && NATIVE.close) return NATIVE.close();
  trace('Back: the window stays');
  if (performance.now() - backHinted > 30000) {
    backHinted = performance.now();
    toast({ title: program ? program.name : 'Lindroids', body: 'Home or the app switcher leaves it running. This window closes when the program ends.' }, 4000);
  }
});

// Once each time the window comes on screen: a user who left full screen (Back, Esc) meant it, and
// asking on every tap took the taps (the resize closed the menu a tap opened).
// When: the earliest input the browser lets ask. Measured in Custom Tabs of Cromite 153 and Firefox
// 158 (lab/browser-window/fsprobe.py, docs/DISPLAY.md): never before the user's first input (not
// while loading, not from the intent that opened the window, not after the page scrolls itself),
// never on a touch's start (pointerdown, touchstart: refused), first on its end (pointerup); a
// mouse button counts from its press, a key from its press. Coming back from Home or Recents, the
// browser has left full screen: the next input asks again. A refused request is asked again on the
// next input, until one is granted.
let wantFull = true, askingFull = false, fullRefusals = 0;
document.addEventListener('visibilitychange', () => { if (!document.hidden) wantFull = true; });
// A key that may ask: a plain key typed on a hardware keyboard, not one of the on-screen
// keyboard's (its window resizes with the screen and would close), nor a shortcut (Ctrl+V needs
// the key's activation for the clipboard).
const fullKey = (e) => !e.isComposing && e.key !== 'Unidentified' && e.key !== 'Process' && e.key !== 'Escape' &&
  !e.ctrlKey && !e.altKey && !e.metaKey && document.activeElement !== ime;
if (OWN_WINDOW) {
  const full = (e) => {
    // Not once the program has ended: the browser's close button is the way out then.
    if (ended !== null && !windows.size) return;
    if (e.target instanceof Element && e.target.closest('.dialog, .toast, .picker')) return;
    if (!wantFull || askingFull || document.fullscreenElement) return;
    if (e.type === 'pointerdown' && e.pointerType !== 'mouse') return;
    if (e.type === 'keydown' && !fullKey(e)) return;
    askingFull = true;
    document.documentElement.requestFullscreen({ navigationUI: 'hide' }).then(() => {
      wantFull = false;
      trace(`full screen asked on ${e.type}`);
    }, (err) => {
      if (fullRefusals++ < 5) trace(`full screen refused on ${e.type}: ${err && err.message}`);
    }).finally(() => { askingFull = false; });
  };
  for (const ev of ['pointerdown', 'pointerup', 'touchend', 'click', 'keydown']) addEventListener(ev, full, { capture: true });
  // What the window looked like when it came: the lab checks it opens out of full screen (no
  // browser allows more) and that the first tap takes it.
  addEventListener('load', () => setTimeout(() => trace(`opened ${document.fullscreenElement ? 'in' : 'out of'} full screen, ${innerWidth}x${innerHeight} of ${screen.width}x${screen.height}`), 1000));
}
$('fs').onclick = () => {
  if (document.fullscreenElement) document.exitFullscreen();
  else document.documentElement.requestFullscreen({ navigationUI: 'hide' }).catch(() => {});
};
// Another window of this program (a run's own window only): the server starts one more instance.
$('neww').hidden = !(OWN_WINDOW && RUN);
$('neww').onclick = () => send({ t: 'new_instance' });

// ------------------------------------------------------------------ clipboard

let lastClipboard = null;

function clipboardFromLinux(text) {
  lastClipboard = text;
  if (NATIVE && NATIVE.writeClipboard) { try { NATIVE.writeClipboard(text); } catch (e) { plog(`clipboard: ${e}`); } return; }
  if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).catch(() => {});
}

/// Hand the browser's clipboard to the Linux side, if it changed. Reading it needs the browser's
/// permission (asked once, on a tap) or a tap right now.
async function clipboardToLinux() {
  if (!ready) return false;
  if (NATIVE && NATIVE.readClipboard) {
    try {
      const text = NATIVE.readClipboard();
      if (text && text !== lastClipboard) {
        lastClipboard = text;
        send({ t: 'clipboard', text });
      }
      return true;
    } catch (e) {
      return false;
    }
  }
  if (!navigator.clipboard || !navigator.clipboard.readText) return false;
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
  if (NATIVE) return clipboardToLinux();
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
// What happens to this window, in the server's log (`lindroids log`): what a phone's browser does
// can only be seen there. Kept while unlinked and sent once linked again.
const traced = [];
function trace(what) {
  if (ready) return plog(`trace: ${what}`);
  if (traced.length < 50) traced.push(`${new Date().toISOString().slice(11, 23)} UTC ${what}`);
}
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
  if (NATIVE && NATIVE.downloadStart) return downloadNative(url, name);
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
// The window app's Android pickers: the URI (and its token in the app) of every file or folder
// picked through the bridge. Reading and writing go back through the app, in chunks.
const nativeKeys = new Map(); // uri -> key in the app's picker
let nativePickSeq = 0;

function b64FromBytes(bytes) {
  let all = '';
  for (let i = 0; i < bytes.length; i += 0x8000) {
    all += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return btoa(all);
}

function bytesFromB64(s) {
  const b = atob(s || '');
  const out = new Uint8Array(b.length);
  for (let i = 0; i < b.length; i++) out[i] = b.charCodeAt(i);
  return out;
}

// Answers from the app's picker and downloads. The app calls these on the page's own thread.
window.__lindroidsPicked = (request, result) => {
  const id = Number(request);
  if (!result || result.cancel) return send({ t: 'files_picked', request: id, files: [] });
  if (result.error) {
    plog(`picker: ${result.error}`);
    return send({ t: 'files_picked', request: id, files: [] });
  }
  const files = (result.entries || []).map((e) => {
    nativeKeys.set(e.uri, e.key);
    return { uri: e.uri, name: e.name, size: e.size || 0, mime: e.mime, rel: e.rel, writable: !!e.writable };
  });
  send({ t: 'files_picked', request: id, files });
};
window.__lindroidsSaved = (token, result) => {
  if (!result || !result.ok) plog(`download: ${result && result.error ? result.error : 'not saved'}`);
};

// Android's own picker through the window app. It answers asynchronously, through
// window.__lindroidsPicked, so nothing here waits on a tap.
function pickNative(p) {
  const kind = p.directory ? 'dir' : p.save ? 'save' : 'open';
  const req = String(p.request || ++nativePickSeq);
  try {
    NATIVE.pickFiles(req, kind, !!p.multiple, JSON.stringify(p.mime_types || []), p.name || '');
  } catch (e) {
    plog(`picker: ${e}`);
    send({ t: 'files_picked', request: p.request, files: [] });
  }
}

// A file picked through the app, in chunks of half a megabyte (the bridge's strings).
function fetchNative(m, key) {
  const CHUNK = 512 * 1024;
  let off = 0;
  for (;;) {
    const part = NATIVE.readPicked(key, off);
    if (part === null || part === undefined) {
      return send({ t: 'file_chunk', request: m.request, index: m.index, last: true, error: 'the file could not be read', len: 0 });
    }
    const bytes = bytesFromB64(part);
    const last = bytes.length < CHUNK;
    send({ t: 'file_chunk', request: m.request, index: m.index, last, len: bytes.length }, bytes);
    if (last) return;
    off += bytes.length;
  }
}

// A save the page could not hand to the app as a picked document (a blob from a program's Save).
async function downloadNative(url, name) {
  try {
    const blob = await (await fetch(url)).blob();
    const token = NATIVE.downloadStart(name || 'file');
    if (!token) throw new Error('the app did not start a download');
    const CHUNK = 512 * 1024;
    for (let off = 0; off < blob.size || off === 0; off += CHUNK) {
      const part = new Uint8Array(await blob.slice(off, off + CHUNK).arrayBuffer());
      const last = off + CHUNK >= blob.size;
      const r = NATIVE.downloadChunk(token, b64FromBytes(part), last);
      if (r !== 'ok') throw new Error(r);
      if (last) break;
    }
  } catch (e) {
    plog(`download: ${e}`);
  }
}
const writes = new Map(); // uri -> the write going on (a promise of its stream; null when done)

// The last tap or click on the page, for what a browser allows only right after one.
let lastTap = -1e9;
addEventListener('pointerup', () => { lastTap = performance.now(); }, { capture: true });

// A program's file dialog first asks where the file is: in the Linux container (browsed here, the
// program reads the path directly) or in Android (Android's own picker).
function pickFiles(p) {
  const title = p.title || (p.directory ? 'A Linux program wants a folder' : p.save ? 'Save' : 'A Linux program wants a file');
  const cancel = () => send({ t: 'files_picked', request: p.request, files: [] });
  pickerSheet(title, [
    ['Linux environment', 'Programs and files in the container', () => browseLinux(p, 'root')],
    ['Home directory', 'Your home in the container', () => browseLinux(p, '~')],
    ['Android files', 'Photos, downloads and folders from Android', () => pickAndroid(p)],
  ], cancel);
}

function pickerSheet(title, options, cancel) {
  const el = document.createElement('div');
  el.className = 'picker';
  el.innerHTML = `<div class="sheet" role="dialog" aria-modal="true">
    <div class="head"><h2>${esc(title)}</h2><button class="x" aria-label="Close">✕</button></div>
    <div class="sources"></div></div>`;
  let settled = false;
  const close = () => { if (settled) return; settled = true; el.remove(); cancel(); };
  el.querySelector('.x').onclick = close;
  el.onclick = (e) => { if (e.target === el) close(); };
  for (const [label, sub, fn] of options) {
    const b = document.createElement('button');
    b.innerHTML = `${esc(label)}<small>${esc(sub)}</small>`;
    b.onclick = () => { if (settled) return; settled = true; el.remove(); fn(); };
    el.querySelector('.sources').appendChild(b);
  }
  document.body.appendChild(el);
  addEventListener('keydown', function esc_(e) {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    removeEventListener('keydown', esc_, true);
    close();
  }, true);
  return el;
}

// The Linux side's own files: the server lists a directory (`/ls`), the program gets the path.
function browseLinux(p, start) {
  const el = document.createElement('div');
  el.className = 'picker';
  el.innerHTML = `<div class="sheet" role="dialog" aria-modal="true">
    <div class="head"><h2>${esc(p.title || (p.save ? 'Save' : p.directory ? 'Choose a folder' : 'Choose a file'))}</h2><button class="x" aria-label="Close">✕</button></div>
    <div class="where"><button class="up" title="Up" aria-label="Up">↑</button><span class="path"></span><button class="home">Home</button></div>
    <div class="list"></div><div class="foot"></div></div>`;
  document.body.appendChild(el);
  const pathEl = el.querySelector('.path'), listEl = el.querySelector('.list'), footEl = el.querySelector('.foot');
  const upBtn = el.querySelector('.up');
  let here = null, selected = new Set(), done = false, actionBtn = null, nameInput = null, footBuilt = false;
  const cancel = () => { if (!done) send({ t: 'files_picked', request: p.request, files: [] }); };
  const close = () => { done = true; el.remove(); cancel(); };
  el.querySelector('.x').onclick = close;
  el.onclick = (e) => { if (e.target === el) close(); };
  upBtn.onclick = () => { if (here && here.parent) go(here.parent); };
  el.querySelector('.home').onclick = () => go('~');

  function join(dir, name) { return (dir.endsWith('/') ? dir : dir + '/') + name; }
  function picked(path, name, mime, size) {
    return { uri: 'linux:' + path, name, path, mime, size: size || 0, writable: true, ...(mime === 'inode/directory' ? { rel: '' } : {}) };
  }
  function choose(files) {
    if (done || !files.length) return;
    done = true;
    el.remove();
    send({ t: 'files_picked', request: p.request, files });
  }
  function chooseDir() { if (here) choose([picked(here.path, here.path.split('/').filter(Boolean).pop() || '/', 'inode/directory')]); }
  function chooseSave() {
    if (!here) return;
    const name = (nameInput.value || '').trim().replace(/[/\0]/g, '_');
    if (!name || name === '.' || name === '..') { nameInput.focus(); return; }
    choose([picked(join(here.path, name), name)]);
  }
  function chooseSelected() { choose([...selected].sort().map((n) => picked(join(here.path, n), n))); }

  function buildFoot() {
    if (footBuilt) return;
    footBuilt = true;
    const add = (label, fn, ghost) => {
      const b = document.createElement('button');
      b.textContent = label;
      if (ghost) b.className = 'ghost';
      b.onclick = fn;
      footEl.appendChild(b);
      return b;
    };
    if (p.directory) {
      actionBtn = add('Use this folder', chooseDir);
    } else if (p.save) {
      nameInput = document.createElement('input');
      nameInput.value = p.name || 'file';
      nameInput.setAttribute('aria-label', 'File name');
      nameInput.onkeydown = (e) => { if (e.key === 'Enter') chooseSave(); };
      footEl.appendChild(nameInput);
      actionBtn = add('Save here', chooseSave);
      actionBtn.disabled = !here.writable;
    } else if (p.multiple) {
      actionBtn = add('Choose', chooseSelected);
    } else {
      footEl.hidden = true;
    }
  }
  function updateFoot() {
    if (actionBtn && p.save) actionBtn.disabled = !here.writable;
    if (actionBtn && p.multiple && !p.directory && !p.save) {
      actionBtn.textContent = selected.size ? `Choose ${selected.size}` : 'Choose';
      actionBtn.disabled = !selected.size;
    }
  }

  async function go(path) {
    listEl.innerHTML = '<div class="empty">…</div>';
    try {
      const r = await fetch(`/ls?t=${encodeURIComponent(PICKER_TOKEN)}&path=${encodeURIComponent(path)}`);
      const j = await r.json();
      if (j.error) {
        listEl.innerHTML = `<div class="err">${esc(j.error)}</div>`;
        return;
      }
      here = j;
      selected = new Set();
      pathEl.textContent = j.path;
      pathEl.title = j.path;
      upBtn.disabled = !j.parent;
      buildFoot();
      updateFoot();
      render();
    } catch (e) {
      listEl.innerHTML = `<div class="err">${esc(e)}</div>`;
    }
  }

  function sizeText(n) {
    if (n < 1024) return n + ' B';
    if (n < 1024 * 1024) return Math.round(n / 1024) + ' KB';
    if (n < 1024 * 1024 * 1024) return (n / (1024 * 1024)).toFixed(1) + ' MB';
    return (n / (1024 * 1024 * 1024)).toFixed(1) + ' GB';
  }

  function render() {
    listEl.textContent = '';
    if (!here.entries.length) {
      listEl.innerHTML = '<div class="empty">This folder is empty</div>';
      return;
    }
    for (const e of here.entries) {
      const row = document.createElement('button');
      row.className = 'row' + (selected.has(e.name) ? ' sel' : '');
      row.innerHTML = `<span class="i">${e.dir ? '▸' : ''}</span><span class="n"></span><span class="s">${e.dir ? '' : sizeText(e.size)}</span>`;
      row.querySelector('.n').textContent = e.name;
      if (e.hidden) row.style.color = 'var(--dim)';
      row.onclick = () => {
        if (e.dir) {
          go(join(here.path, e.name));
        } else if (p.save) {
          nameInput.value = e.name;
        } else if (p.directory) {
          return;
        } else if (p.multiple) {
          if (selected.has(e.name)) selected.delete(e.name);
          else selected.add(e.name);
          row.classList.toggle('sel');
          updateFoot();
        } else {
          choose([picked(join(here.path, e.name), e.name, e.mime, e.size)]);
        }
      };
      listEl.appendChild(row);
    }
  }

  addEventListener('keydown', function esc_(e) {
    if (e.key !== 'Escape') return;
    e.preventDefault();
    removeEventListener('keydown', esc_, true);
    close();
  }, true);
  go(start);
}

// Android's picker: the one the browser has, over Android's documents (it needs a tap).
function pickAndroid(p) {
  const cancel = () => send({ t: 'files_picked', request: p.request, files: [] });
  if (NATIVE && NATIVE.pickFiles) return pickNative(p);
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
  const nk = nativeKeys.get(m.uri);
  if (nk) return fetchNative(m, nk);
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
  const nk = nativeKeys.get(m.uri);
  if (nk) {
    const r = NATIVE.writePicked(nk, b64FromBytes(payload), m.offset || 0, !!m.last);
    if (r !== 'ok') return send({ t: 'write_result', request: m.request, ok: false, message: String(r) });
    if (m.last) send({ t: 'write_result', request: m.request, ok: true });
    return;
  }
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

// Whether the page is on screen: a page the browser drops while it is out of sight (Android kills
// a background page's process) was not closed, and its program keeps running until it is back.
document.addEventListener('visibilitychange', () => {
  trace(document.hidden ? 'hidden' : 'shown');
  if (ready) send({ t: 'shown', shown: !document.hidden });
});
addEventListener('pageshow', (e) => trace(`pageshow${e.persisted ? ' (from the back/forward cache)' : ''}`));
document.addEventListener('freeze', () => trace('frozen'));
document.addEventListener('resume', () => trace('resumed'));
document.addEventListener('fullscreenchange', () => trace(document.fullscreenElement ? 'full screen' : 'out of full screen'));

// The page is going away for good (its window closed, or a reload): say so, so the server does
// not wait a minute for a browser that might come back. A page put in the back/forward cache
// (persisted) is not going away.
addEventListener('pagehide', e => {
  trace(`pagehide${e.persisted ? ' (kept in the back/forward cache)' : ''}`);
  if (!e.persisted && ready) send({ t: 'closing' });
});

// ------------------------------------------------------------------ start

if (!window.isSecureContext || !crypto.subtle) {
  showPanel(`<p class="err">Open this page as <code>http://127.0.0.1:${esc(location.port)}/</code>: the browser only allows what Lindroids needs there.</p>`);
} else {
  connect();
}
