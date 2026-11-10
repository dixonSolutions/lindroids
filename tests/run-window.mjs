#!/usr/bin/env node
// A run's window and its run, from the page's side (docs/DISPLAY.md, "Lifecycles"): a window
// brought back after its run ended (its ticket spent) must say so and close, or, in a program's
// own app, start the program again; never sit on a dead panel. Also: the page tells the window app
// which run it shows, and says once when the app is older than its server. Drives
// crates/lindroids-server/data/web/lindroids.js in headless Chromium, the link replaced by a
// recorder and the window app's bridge (window.lindroids) by a fake, as tests/stuck-input.mjs does.
//
//   node tests/run-window.mjs [--js <lindroids.js>]
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const webDir = resolve(here, '../crates/lindroids-server/data/web');
const argOf = (flag) => { const i = process.argv.indexOf(flag); return i > 0 ? process.argv[i + 1] : null; };
const jsPath = resolve(argOf('--js') || process.env.LINDROIDS_JS || join(webDir, 'lindroids.js'));

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

// The template number this server's page expects (lindroids_proto::APP_TEMPLATE).
const TEMPLATE = Number(/pub const APP_TEMPLATE: u32 = (\d+);/.exec(readFileSync(resolve(here, '../crates/lindroids-proto/src/lib.rs'), 'utf8'))[1]);

function serve() {
  const js = readFileSync(jsPath, 'utf8').replace('@BUILD@', 'test').replace('@PICKER_TOKEN@', 'test-picker').replace('@APP_TEMPLATE@', String(TEMPLATE));
  const html = readFileSync(join(webDir, 'index.html'));
  const files = {
    '/lindroids.js': ['text/javascript; charset=utf-8', js],
    '/draw.js': ['text/javascript; charset=utf-8', readFileSync(join(webDir, 'draw.js'))],
    '/icon.svg': ['image/svg+xml', readFileSync(join(webDir, 'icon.svg'))],
  };
  return new Promise((resolveServer) => {
    const server = createServer((req, res) => {
      const path = new URL(req.url, 'http://x').pathname;
      // Every page (/, a program's /app/<id>/) is the same index.html, whose script is absolute.
      const f = files[path] || (path === '/' || /^\/app\/[a-z.]+\/$/.test(path) ? ['text/html; charset=utf-8', html] : null);
      if (!f) { res.writeHead(404); return res.end('not found'); }
      res.writeHead(200, { 'content-type': f[0], 'cache-control': 'no-store' });
      res.end(f[1]);
    });
    server.listen(0, '127.0.0.1', () => resolveServer(server));
  });
}

// Before the page's scripts: the recorder of the link, and the window app's bridge, whose
// template() is there unless the URL says `tmpl=none` (an app from before it), and answers
// `tmpl=<n>` otherwise.
const INIT = `(() => {
  window.__sent = [];
  window.__sockets = [];
  window.__native = [];
  class FakeSocket {
    constructor(url) {
      this.readyState = 0;
      this.index = window.__sockets.length;
      window.__sockets.push(this);
      setTimeout(() => { this.readyState = 1; if (this.onopen) this.onopen({}); }, 0);
    }
    send(data) {
      if (typeof data === 'string') {
        for (const line of data.split('\\n')) if (line) window.__sent.push({ socket: this.index, ...JSON.parse(line) });
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
  window.__deliver = (msg) => window.__sockets[window.__sockets.length - 1].onmessage({ data: new TextEncoder().encode(JSON.stringify(msg)).buffer });
  const q = new URLSearchParams(location.search);
  if (q.has('native')) {
    const call = (name) => (...args) => { window.__native.push([name, ...args]); };
    window.lindroids = {
      close: call('close'), attached: call('attached'), detached: call('detached'),
      packageName: () => q.get('native'),
    };
    if (q.get('tmpl') !== 'none') window.lindroids.template = () => Number(q.get('tmpl'));
  }
})();`;

const results = [];
function record(name, pass, detail) {
  results.push({ name, pass, detail });
  console.log(`${pass ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
}

async function open(browser, server, path) {
  const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
  await context.addInitScript(INIT);
  const page = await context.newPage();
  page.on('pageerror', (e) => console.log(`  [page error] ${e.message}`));
  await page.goto(`http://127.0.0.1:${server.address().port}${path}`);
  await page.waitForFunction(() => window.__sent.some((m) => m.t === 'hello'));
  return { context, page };
}

const SPENT = { t: 'auth_failed', reason: 'this window was not opened by lindroids run', pairing_open: true };
const panelText = (page) => page.evaluate(() => (document.getElementById('panel').hidden ? '' : document.getElementById('panel').innerText));

// A window of the shared app, its task rebuilt from Recents after its run ended: the end, then
// closed (never the "not opened by lindroids run" panel, which nothing ever leaves).
async function caseSpentShared(browser, server) {
  const { context, page } = await open(browser, server, `/?window&run=4242&native=org.lindroids.windows&tmpl=${TEMPLATE}#t=spent`);
  await page.evaluate((m) => window.__deliver(m), SPENT);
  await page.waitForTimeout(1800);
  const text = await panelText(page);
  const native = await page.evaluate(() => window.__native.map((c) => c[0]));
  const pass = /closed/.test(text) && !/not opened/.test(text) && native.includes('close') && native.includes('detached');
  record('1 spent ticket in the windows app: the end, then the window closes', pass, `panel "${text.replace(/\s+/g, ' ').trim()}", bridge ${native.join(',')}`);
  await context.close();
}

// The same in a browser's Custom Tab, never linked before on this page (no history state).
async function caseSpentBrowser(browser, server) {
  const { context, page } = await open(browser, server, '/?window&run=4242#t=spent');
  await page.evaluate((m) => window.__deliver(m), SPENT);
  await page.waitForTimeout(300);
  const text = await panelText(page);
  const pass = /closed/.test(text) && !/not opened/.test(text);
  record('2 spent ticket in a browser window: the end, not a dead panel', pass, `panel "${text.replace(/\s+/g, ' ').trim()}"`);
  await context.close();
}

// A program's own app: back to its start page, which starts the program again.
async function caseSpentApp(browser, server) {
  const { context, page } = await open(browser, server, `/app/demo/?window&run=4242&native=org.lindroids.app.demo&tmpl=${TEMPLATE}#t=spent`);
  await page.evaluate((m) => window.__deliver(m), SPENT);
  await page.waitForURL((u) => u.pathname === '/app/demo/' && u.search === '', { timeout: 3000 }).catch(() => {});
  const url = new URL(page.url());
  record('3 spent ticket in a program\'s own app: back to its start page', url.pathname === '/app/demo/' && url.search === '', page.url());
  await context.close();
}

// Linked: the app learns the run and ticket, and the hello names the app and its template.
async function caseAttached(browser, server) {
  const { context, page } = await open(browser, server, `/?window&run=77&native=org.lindroids.windows&tmpl=${TEMPLATE}#t=good`);
  const hello = await page.evaluate(() => window.__sent.find((m) => m.t === 'hello'));
  await page.evaluate(() => window.__deliver({ t: 'hello' }));
  await page.waitForTimeout(100);
  const native = await page.evaluate(() => window.__native);
  const toasts = await page.evaluate(() => document.getElementById('toasts').innerText);
  const attached = native.find((c) => c[0] === 'attached');
  const pass = attached && attached[1] === '77' && attached[2] === 'good' && JSON.stringify(hello.app) === JSON.stringify(['org.lindroids.windows', TEMPLATE]) && !/out of date/.test(toasts);
  record('4 linked: the app is told its run and ticket; an app up to date is not called out of date', !!pass,
    `attached ${JSON.stringify(attached)}, hello.app ${JSON.stringify(hello.app)}, toasts "${toasts}"`);
  // The run ends: the page tells the app it shows no run any more.
  await page.evaluate(() => window.__deliver({ t: 'run_ended', code: 0 }));
  await page.waitForTimeout(100);
  const after = await page.evaluate(() => window.__native.map((c) => c[0]));
  record('5 the run ended: the app is told (detached)', after.includes('detached'), after.join(','));
  await context.close();
}

// An app from before the template had a number (its bridge has no template()): told once.
async function caseStale(browser, server) {
  for (const [app, how] of [['/?window&run=5&native=org.lindroids.windows&tmpl=none#t=x', 'lindroids app install'],
    ['/app/demo/?window&run=5&native=org.lindroids.app.demo&tmpl=0#t=x', 'lindroids export demo']]) {
    const { context, page } = await open(browser, server, app);
    await page.evaluate(() => window.__deliver({ t: 'hello' }));
    await page.waitForTimeout(100);
    const toasts = await page.evaluate(() => document.getElementById('toasts').innerText);
    const hello = await page.evaluate(() => window.__sent.find((m) => m.t === 'hello'));
    const logs = await page.evaluate(() => window.__sent.filter((m) => m.t === 'log').map((m) => m.msg).join(' | '));
    const pass = /out of date/.test(toasts) && toasts.includes(how) && hello.app[1] === 0 && logs.includes(how);
    record(`6 an app older than the server's template is called out of date (${how})`, pass, `toasts "${toasts.replace(/\s+/g, ' ')}", hello.app ${JSON.stringify(hello.app)}`);
    await context.close();
  }
}

// Going to another of its program's pages in the same window (`navigate`) is no closing.
async function caseNavigateNoClosing(browser, server) {
  const { context, page } = await open(browser, server, '/app/demo/?native=org.lindroids.app.demo&tmpl=1');
  await page.evaluate(() => window.__deliver({ t: 'hello' }));
  // Keep what the page sends across the navigation.
  await page.evaluate(() => addEventListener('pagehide', () => sessionStorage.setItem('sent', JSON.stringify(window.__sent))));
  await page.evaluate(() => window.__deliver({ t: 'navigate', url: '/app/demo/?window&run=9#t=y' }));
  await page.waitForURL((u) => u.search.includes('run=9'), { timeout: 3000 }).catch(() => {});
  const before = await page.evaluate(() => JSON.parse(sessionStorage.getItem('sent') || '[]'));
  const pass = page.url().includes('run=9') && !before.some((m) => m.t === 'closing');
  record('7 in-app navigation does not say the window is closing', pass, `${page.url()}, sent ${before.map((m) => m.t).join(',')}`);
  await context.close();
}

const server = await serve();
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
try {
  console.log(`testing ${jsPath} (template ${TEMPLATE})`);
  await caseSpentShared(browser, server);
  await caseSpentBrowser(browser, server);
  await caseSpentApp(browser, server);
  await caseAttached(browser, server);
  await caseStale(browser, server);
  await caseNavigateNoClosing(browser, server);
} finally {
  await browser.close();
  server.close();
}
const failed = results.filter((r) => !r.pass).length;
console.log(`${results.length - failed}/${results.length} passed`);
process.exit(failed ? 1 : 0);
