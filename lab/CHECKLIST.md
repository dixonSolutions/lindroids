# What "fully working" means

Each line was checked on the lab VM b16 (Android 13, SELinux enforcing, Firefox as the window
browser; GNOME Calculator and Text Editor from Flathub, Mousepad and foot from Termux) unless it
says otherwise. ✅ works, 🟡 partly, ❌ not yet, ⛔ not possible without an APK (and why).

The split: **the program, its data and its shell live in the container** (Termux's prefix, a
proot distro, a Flatpak deploy). **Files and folders outside it, system state and hardware come
from Android**, through the window page and Termux, never by giving the container the device.

## Window

| Criterion | Status | Evidence / notes |
|---|---|---|
| One program, one window of its own in Recents (not a tab) | ✅ | Custom Tab in its own task |
| The program fills the window | ✅ | no tab strip for one window; tools behind a handle at the top; dialogs bring the tabs back |
| Window shows the program's name and icon | 🟡 | title, start/ended screens, favicon; ⛔ Recents shows the browser's icon (Firefox sets its task icon itself; Chrome-family untested) |
| Full screen, no browser bars | 🟡 | after the first tap (browsers allow full screen only on a tap) |
| Follows Android's dark mode and accent colour | ✅ | Settings portal; libadwaita, GTK 3 |
| Program's icons and title bar draw right (Adwaita) | ✅ | portal sends GNOME's icon theme and button layout |
| Closing the program ends the window | 🟡 | ended screen with the program's icon, out of full screen; ⛔ Firefox ignores `window.close()`: one tap on ✕ |
| Closing the window (✕, swipe from Recents) ends the program | ✅ | |
| Back does not lose the program | ✅ | Back only sends the window behind |

## Launch and export

| Criterion | Status | Evidence / notes |
|---|---|---|
| `lindroids run <app>` from Termux opens the window | ✅ | |
| `lindroids export <app>` pins a home-screen icon with the program's icon and name | ✅ | adaptive icon; Android badges it with Termux (every pinned shortcut shows its owner) |
| Exporting again updates the icon in place | ✅ | no second icon |
| Shortcut works with Termux closed | ✅ | [demo](../media/demos/launch-from-home.mp4) |
| Shortcut works right after a reboot, Termux never opened | ✅ | window in ~1–2 s, enforcing |
| The window says the program ended even when Termux closes with it | ✅ | |

## Android → program (piped in)

| Criterion | Status | Evidence / notes |
|---|---|---|
| Open a file from Android (Files "Open with" / Share → Termux → Edit) in a Linux program | ✅ | `termux-file-editor` → `lindroids open --linux`; [demo](../media/demos/open-with.mp4); edits go to Termux's copy in `~/downloads` |
| File > Open in a program shows Android's picker (Files, Camera) | ✅ | portal programs (GTK 4, Flatpak); straight to the picker after the tap; [demo](../media/demos/file-picker.mp4); older GTK 3 dialogs browse Termux's files |
| Edits reach the picked original | ⛔ | Android browsers cannot write back to a picked file; Save As goes to Downloads |
| Save (Save As) goes to Android's Downloads, again and again | ✅ | |
| Folders (pick a folder) | ⛔ | no folder picker in Android browsers; `/sdcard` is there when Termux has storage access |
| Linux files visible to Android apps | ✅ | Termux's storage provider in Files (`~/downloads`, Flatpak data under `~/.var/app`) |
| Notifications are Android notifications | ✅ | posted as Termux, with the program's name and icon; tap brings back its window |
| Requests reach the program's own window | ✅ | sound, pickers, links, notifications by pid → run |
| Links open in Android's browser | ✅ | |
| Clipboard both ways (text) | ✅ | images ❌ |
| Sound out | 🟡 | per program; the VM has no audio output to check by ear |
| Camera (as a photo for a program) | ✅ | through the file picker |
| Microphone, live camera, location | ❌ | |
| Language, time zone | 🟡 | time zone from Android for Flatpaks; language: Termux's `LANG` |
| Battery, power saver, network, low memory | 🟡 | network yes; others constants |
| Screen density, rotation | 🟡 | integer scale fixed at connect; rotation resizes |
| Soft keyboard, hardware keyboard | 🟡 | US layout only outside text-input programs |

## Speed (b16, software rendering)

| Criterion | Status | Evidence / notes |
|---|---|---|
| Tap on the icon to a drawn window | ✅ | ~1–2 s (Calculator) |
| Scrolling | ✅ | 55 fps, 8.5 Mpx/s, 1.4 ms to draw a frame (`LINDROIDS_STATS=1`) |
| Typing and taps feel immediate | ✅ | |

## Demos

Recorded with `lab/record.sh` (the VM has no hardware encoder) in `media/demos/`:
`launch-from-home` (icon → Calculator with Termux closed → close), `open-with` (Files → Termux →
Mousepad), `file-picker` (Text Editor → Android's picker).
