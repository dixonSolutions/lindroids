#!/usr/bin/env node
// The folder picker, from the page's side: `lindroids pick-folder` (a CLI request) sends the page
// `pick_folder`, and the page must show a Linux folder chooser and answer `folder_picked` with
// the folder the user chose (or null when cancelled). Drives crates/lindroids-server/data/web/
// lindroids.js in headless Chromium, the link replaced by a recorder, as tests/stuck-input.mjs does.
//
//   node tests/folder-picker.mjs [--js <lindroids.js>] [--screenshot <before-or-after.png>]
//
// The directory listing (`/ls`) is answered here; the server's own listing is tested in Rust.
import { createRequire } from 'node:module';
import { createServer } from 'node:http';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const webDir = resolve(here, '../crates/lindroids-server/data/web');
const argOf = (flag) => { const i = process.argv.indexOf(flag); return i > 0 ? process.argv[i + 1] : null; };
const jsPath = resolve(argOf('--js') || process.env.LINDROIDS_JS || join(webDir, 'lindroids.js'));
const shotPath = argOf('--screenshot') ? resolve(argOf('--screenshot')) : null;

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

// The listing the server would give for `/ls` (names, kinds, paths), for any path asked.
const LISTING = {
  path: '/srv/demo', parent: '/srv', home: '/home/user', root: '/', writable: true,
  entries: [{ name: 'notes.txt', dir: false, hidden: false, size: 12, writable: true }],
};

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
      const url = new URL(req.url, 'http://x');
      if (url.pathname === '/ls') {
        res.writeHead(200, { 'content-type': 'application/json' });
        return res.end(JSON.stringify(LISTING));
      }
      const f = files[url.pathname];
      if (!f) { res.writeHead(404); return res.end('not found'); }
      res.writeHead(200, { 'content-type': f[0], 'cache-control': 'no-store' });
      res.end(f[1]);
    });
    server.listen(0, '127.0.0.1', () => resolveServer(server));
  });
}

// Runs before the page's scripts: the WebSocket records what the page sends and lets the test
// play the server.
const INIT = `(() => {
  window.__sent = [];
  window.__sockets = [];
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
  window.__deliver = (msg) => {
    const s = window.__sockets[window.__sockets.length - 1];
    s.onmessage({ data: new TextEncoder().encode(JSON.stringify(msg)).buffer });
  };
})();`;

const results = [];
function check(name, pass, detail) {
  results.push({ name, pass });
  console.log(`${pass ? 'PASS' : 'FAIL'} ${name}${detail ? `: ${detail}` : ''}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const folderReplies = (page, request) => page.evaluate((r) => window.__sent.filter((m) => m.t === 'folder_picked' && m.request === r), request);

const server = await serve();
const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
let failed = false;
try {
  const context = await browser.newContext({ viewport: { width: 1000, height: 700 } });
  await context.addInitScript(INIT);
  const page = await context.newPage();
  page.on('pageerror', (e) => console.log(`  [page error] ${e.message}`));
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  await page.waitForFunction(() => window.__sent.some((m) => m.t === 'hello'));
  await page.evaluate(() => window.__deliver({ t: 'hello' }));
  await sleep(200);

  // 1. Chosen folder: the picker is shown and its answer is the folder picked.
  await page.evaluate(() => window.__deliver({ t: 'pick_folder', request: 7 }));
  await page.waitForSelector('.picker .sources button', { timeout: 3000 }).catch(() => {});
  const shown = await page.locator('.picker').count();
  check('pick_folder shows a folder chooser on the page', shown > 0, `${shown} picker sheet(s)`);
  check('no answer before a folder is chosen', (await folderReplies(page, 7)).length === 0);
  if (shotPath) await page.screenshot({ path: shotPath });
  if (shown > 0) {
    await page.locator('.picker .sources button', { hasText: 'Linux environment' }).click({ timeout: 3000 }).catch(() => {});
    await page.waitForFunction(() => document.querySelector('.where .path')?.textContent === '/srv/demo', null, { timeout: 3000 }).catch(() => {});
    await page.locator('.picker .foot button', { hasText: 'Use this folder' }).click({ timeout: 3000 }).catch(() => {});
    await sleep(300);
  }
  const chosen = await folderReplies(page, 7);
  check('the chosen folder is answered as folder_picked', chosen.length === 1 && chosen[0].path === '/srv/demo', JSON.stringify(chosen));

  // 2. Cancelled: Escape closes the chooser and the answer says no folder.
  await page.evaluate(() => window.__deliver({ t: 'pick_folder', request: 8 }));
  await page.waitForSelector('.picker', { timeout: 3000 }).catch(() => {});
  await page.keyboard.press('Escape');
  await sleep(300);
  const cancelled = await folderReplies(page, 8);
  check('cancelling answers folder_picked with no path', cancelled.length === 1 && cancelled[0].path === null, JSON.stringify(cancelled));
  check('the chooser is gone after cancelling', (await page.locator('.picker').count()) === 0);
  // 3. Closed with X inside the folder browser: answered with no path as well.
  await page.evaluate(() => window.__deliver({ t: 'pick_folder', request: 9 }));
  await page.waitForSelector('.picker .sources button', { timeout: 3000 }).catch(() => {});
  await page.locator('.picker .sources button', { hasText: 'Linux environment' }).click({ timeout: 3000 }).catch(() => {});
  await page.waitForSelector('.picker .where', { timeout: 3000 }).catch(() => {});
  await page.locator('.picker .head .x').click({ timeout: 3000 }).catch(() => {});
  await sleep(300);
  const closed = await folderReplies(page, 9);
  check('closing the folder browser with X answers folder_picked with no path', closed.length === 1 && closed[0].path === null, JSON.stringify(closed));
  // 4. The file chooser (the portal's FileChooser, as VSCodium's Open Folder asks for it) closed
  // with X: the portal request must be answered, not left to its hour-long timeout.
  const FILE_REQUEST = 1073741830;
  await page.evaluate((r) => window.__deliver({ t: 'pick_files', request: r, title: 'Open folder', save: false, multiple: false, directory: true, mime_types: [] }), FILE_REQUEST);
  await page.waitForSelector('.picker .sources button', { timeout: 3000 }).catch(() => {});
  await page.locator('.picker .sources button', { hasText: 'Linux environment' }).click({ timeout: 3000 }).catch(() => {});
  await page.waitForSelector('.picker .where', { timeout: 3000 }).catch(() => {});
  await page.locator('.picker .head .x').click({ timeout: 3000 }).catch(() => {});
  await sleep(300);
  const filesAnswer = await page.evaluate((r) => window.__sent.filter((m) => m.t === 'files_picked' && m.request === r), FILE_REQUEST);
  check('closing the file chooser with X answers files_picked with no files', filesAnswer.length === 1 && filesAnswer[0].files.length === 0, JSON.stringify(filesAnswer));
  await context.close();
} finally {
  await browser.close();
  server.close();
}
const failures = results.filter((r) => !r.pass).length;
console.log(failures ? `${failures} FAILED` : `all ${results.length} checks passed`);
process.exit(failures ? 1 : 0);
