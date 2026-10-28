// Lindroids: an exported program's own pages (/app/<id>/, `lindroids export --webapk`), installed
// as an app of its own. When the server cannot be reached (Termux is closed), its icon would open
// the browser's error page: this shows one of ours instead, which starts the program through
// Termux. Everything else goes to the server as it is; nothing else is kept.
'use strict';

const CACHE = 'lindroids-app-@BUILD@';
const OFFLINE = ['offline', 'offline.js', 'icon.png'];

self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(OFFLINE)).then(() => self.skipWaiting()));
});

self.addEventListener('activate', (e) => {
  e.waitUntil(caches.keys()
    .then((keys) => Promise.all(keys.filter((k) => k.startsWith('lindroids-app-') && k !== CACHE).map((k) => caches.delete(k))))
    .then(() => self.clients.claim()));
});

// A page that cannot be loaded gets the offline page, and the offline page its script and icon
// (with or without the icon's ?v=).
self.addEventListener('fetch', (e) => {
  if (e.request.method !== 'GET') return;
  const navigate = e.request.mode === 'navigate';
  const path = new URL(e.request.url).pathname;
  if (!navigate && !OFFLINE.some((f) => path === new URL(f, self.registration.scope).pathname)) return;
  e.respondWith(fetch(e.request)
    .catch(() => caches.open(CACHE).then((c) => c.match(navigate ? 'offline' : e.request, { ignoreSearch: true })))
    .then((r) => r || Response.error()));
});
