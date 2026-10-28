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

self.addEventListener('fetch', (e) => {
  if (e.request.mode !== 'navigate') return;
  e.respondWith(fetch(e.request).catch(() => caches.open(CACHE).then((c) => c.match('offline')))
    .then((r) => r || Response.error()));
});
