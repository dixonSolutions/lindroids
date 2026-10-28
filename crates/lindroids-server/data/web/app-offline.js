// The offline page of an exported program's app (app-sw.js): Start shares the program's launch
// URL with Termux, whose share receiver starts Termux and runs `lindroids launch <id>` (as its
// Termux icon does); the window then opens here. Back to the program's page once the server
// answers.
'use strict';

const launch = document.body.dataset.launch;
document.getElementById('start').onclick = () => {
  if (navigator.share) navigator.share({ url: launch }).catch(() => {});
};
setInterval(() => {
  fetch('manifest.webmanifest', { cache: 'no-store' }).then((r) => { if (r.ok) location.replace('./'); }).catch(() => {});
}, 3000);
