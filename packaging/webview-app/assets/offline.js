'use strict';

// The Lindroids window app's own page, shown when the server in Termux cannot be reached. Start
// hands the program's launch URL to Termux as the WebAPK offline page does, but with an explicit
// SEND intent from our own code instead of Android's share sheet. Try again (and a quiet retry
// every few seconds) reloads the page as soon as Termux answers.

const q = new URLSearchParams(location.search);
const name = q.get('name') || 'This program';
const launch = q.get('launch') || '';
document.title = name;
document.getElementById('name').textContent = name;
document.getElementById('what').textContent = name;

document.getElementById('start').onclick = () => {
  if (window.lindroids && window.lindroids.startTermux) window.lindroids.startTermux();
};
document.getElementById('retry').onclick = () => {
  if (window.lindroids && window.lindroids.retry) window.lindroids.retry();
};

if (q.get('auto')) {
  document.getElementById('hint').textContent = `Starting ${name} in Termux…`;
}
if (!launch) {
  document.getElementById('hint').textContent = 'Open Termux and start the program there; this window follows.';
}

setInterval(() => {
  if (window.lindroids && window.lindroids.retry) window.lindroids.retry();
}, 4000);
