# What "fully working" means

Each line is checked on the lab VM b16 (Android 13, SELinux enforcing, Firefox as the window
browser, a Flatpak app and a Termux-native app) unless it says otherwise. ✅ works and was
checked, 🟡 partly, ❌ not yet, ⛔ cannot be done without an APK (and why).

The split: **the program, its data and its shell live in the container** (Termux's prefix, a
proot distro, a Flatpak deploy). **Files and folders outside it, system state and hardware come
from Android**, through the window page and Termux, never by giving the container the device.

## Window

| Criterion | Status | Evidence / notes |
|---|---|---|
| One program, one window of its own in Recents (not a tab) | ✅ | Custom Tab in its own task |
| Window shows the program's name | ✅ | title bar, tab, page title |
| Window shows the program's icon | 🟡 | start/ended screens, tab and favicon; ⛔ Recents shows the browser's icon (Firefox sets the task icon itself) |
| Full screen, no browser bars | 🟡 | after the first tap (browsers allow full screen only on a tap) |
| Follows Android's dark mode and accent colour | ✅ | portal Settings; GTK 4/libadwaita, GTK 3 |
| Program's own icons and title bar draw right (Adwaita) | ✅ | portal sends GNOME's icon theme and button layout |
| Closing the program ends the window | 🟡 | process ends; Firefox ignores `window.close()`: the page leaves full screen and asks for ✕ |
| Closing the window (✕, swipe from Recents) ends the program | ✅ | 5 s after the page says goodbye; 60 s after a bare disconnect |
| Back does not lose the program | ✅ | Back only sends the window behind |

## Launch and export

| Criterion | Status | Evidence / notes |
|---|---|---|
| `lindroids run <app>` from Termux opens the window | ✅ | |
| `lindroids export <app>` pins a home-screen icon with the program's icon and name | ✅ | Android badges it with Termux's icon (every pinned shortcut is badged with its owner) |
| Icon fills the launcher's shape like an app icon | ❌ | shown small in a white circle (legacy bitmap) |
| Shortcut works with Termux closed | ✅ | |
| Shortcut works right after a reboot, Termux never opened | ✅ | 2026-10-27: window in ~1 s, enforcing |
| A failed launch says why where the user looks | 🟡 | message in the Termux session that the shortcut opened |

## Android → program (piped in)

| Criterion | Status | Evidence / notes |
|---|---|---|
| File > Open in a program shows Android's file picker | 🟡 | works; the program gets a copy (edits do not reach the original) |
| Save goes back to Android (Downloads) | 🟡 | first save only (second save fails) |
| Open a file from Android (Files app "Open with" / Share → Termux) in a program | ❌ | |
| Folders (pick a folder) | ⛔ | Android browsers have no folder picker; `/sdcard` is bound when Termux has storage access |
| Clipboard both ways (text) | ✅ | images: ❌ |
| Links open in Android's browser | ✅ | |
| Notifications show as Android notifications | ❌ | only as toasts inside the window |
| Sound out | 🟡 | per page; sound can stop after switching windows |
| Microphone, camera, location | ❌ | |
| Language, time zone, 24-hour clock | 🟡 | time zone for Flatpak only; language not passed |
| Battery, power saver, network, low memory | 🟡 | network yes; others constants |
| Screen density, rotation | 🟡 | integer scale fixed at connect; rotation resizes |
| Soft keyboard, hardware keyboard | 🟡 | US layout only outside text-input programs |

## Speed

| Criterion | Status | Evidence / notes |
|---|---|---|
| Tap on the icon to a drawn window | ✅ | ~1–2 s (Calculator, Flatpak, b16) |
| Typing and taps feel immediate | ✅ | |
| Scrolling/animation smooth | 🟡 | to be measured |

## Demos

Recorded with `lab/record.sh` (the VM has no hardware encoder); in `media/demos/`.
