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
| Window shows the program's name and icon | 🟡 | title, start/ended screens, favicon (served at `/run-icon/<run>.png`); Chromium Custom Tabs: Recents shows the program's icon and name (b16, Cromite 153); ⛔ Firefox: the browser's icon |
| The program's icon in the status bar while it runs | ✅ | its `-symbolic` icon (else a window glyph); tap → its window; goes when it ends, and after a killed server |
| Full screen, no browser bars | 🟡 | after the first tap (browsers allow full screen only on a tap) |
| Follows Android's dark mode and accent colour | ✅ | Settings portal; libadwaita, GTK 3 |
| Program's icons and title bar draw right (Adwaita) | ✅ | portal sends GNOME's icon theme and button layout |
| Closing the program ends the window | 🟡 | Chromium Custom Tabs: the window closes itself and leaves Recents, whatever the default browser (b16: Firefox the default, Cromite's window; lab test); ⛔ with only Firefox: the ended screen asks for one tap on ✕ |
| Closing the window ends the program | ✅ | ✕ → 5 s (Firefox, Cromite; lab test); Back only sends a Chromium window to the background, alive |
| Tapping the exported icon again brings the window back | ✅ | Firefox raises its window; Chromium opens a new one, which takes over the run while the old one closes itself (`lab/test-windows.sh`, b16) |
| Recents icon right after opening | 🟡 | Android writes the task's icon a few seconds after the browser sets it: Recents opened in the first seconds shows the browser's icon (b16) |
| Recents icon on Samsung (One UI) | ⛔ | Chrome sets the page's title and icon on the task; One UI shows the title and Chrome's own icon (a phone, 2026-10-28) |
| Any default browser; Custom Tabs not assumed | ✅ | the helper reads which activity a browser opened (`startActivityAndWait`); an ordinary tab (Cromite with *Allow custom tab intents* off) is passed over and closes itself (b16 lab test: Firefox window) |
| Closing the window (✕, swipe from Recents) ends the program | ✅ | |
| Back does not lose the program | ✅ | Back only sends the window behind |
| Coming back to a window that left full screen in the background | 🟡 | Firefox lays the page under its toolbar until the next tap (which goes full screen again) |

## Launch and export

| Criterion | Status | Evidence / notes |
|---|---|---|
| `lindroids run <app>` from Termux opens the window | ✅ | |
| `lindroids export <app>` pins a home-screen icon with the program's icon and name | ✅ | adaptive icon; Android badges it with Termux (every pinned shortcut shows its owner) |
| Exporting again updates the icon in place | ✅ | no second icon |
| Shortcut works with Termux closed | ✅ | [demo](../media/demos/launch-from-home.mp4) |
| Shortcut works right after a reboot, Termux never opened | ✅ | window in ~1–2 s, enforcing |
| The window says the program ended even when Termux closes with it | ✅ | |
| A program that exits before any window says so in its window | ✅ | "ended before showing a window (exit code N)" |
| The window opens even when Android's reply trips over noted app ops | ✅ | helper treats `SyncNotedAppOp` reply errors as done (the start went through) |

## Android → program (piped in)

| Criterion | Status | Evidence / notes |
|---|---|---|
| Open a file from Android (Files "Open with" / Share → Termux → Edit) in a Linux program | ✅ | `termux-file-editor` → `lindroids open --linux`; [demo](../media/demos/open-with.mp4); edits go to Termux's copy in `~/downloads` |
| File > Open in a program shows Android's picker (Files, Camera) | ✅ | portal programs (GTK 4, Flatpak); straight to the picker after the tap; [demo](../media/demos/file-picker.mp4); older GTK 3 dialogs browse Termux's files |
| Edits reach the picked original | 🟡 | Chromium-based browsers (File System Access): Text Editor open → edit → Save wrote `/sdcard/Documents/meeting.txt`, one "Save changes?" prompt, later saves without one (b16, Cromite 153). ⛔ Firefox: a copy |
| Save As goes where the user chose, again and again | 🟡 | Chromium-based: Android's save picker, then each save into that file (b16, Cromite). Firefox: to Downloads ✅ |
| Folders (pick a folder) | 🟡 | Chromium-based: FileChooser `directory` → Android's folder picker; copied to Linux, a changed file and a new file in a new subfolder written back (b16, Cromite, via `gdbus`); deletions are not. ⛔ Firefox; `/sdcard` is there when Termux has storage access |
| Linux files visible to Android apps | ✅ | Termux's storage provider in Files (`~/downloads`, Flatpak data under `~/.var/app`) |
| Notifications are Android notifications | ✅ | posted as Termux, with the program's name and icon; tap brings back its window |
| Requests reach the program's own window | ✅ | sound, pickers, links, notifications by pid → run |
| A program can keep the screen on (Inhibit portal) | ✅ | page's screen wake lock; Android held it exactly while the program inhibited |
| Links open in Android's browser | ✅ | |
| Clipboard both ways (text) | ✅ | images ❌ |
| Sound out | 🟡 | per program; the VM has no audio output to check by ear |
| Camera (as a photo for a program) | ✅ | through the file picker |
| Live camera, location | ❌ | |
| Language, time zone | 🟡 | time zone from Android for Flatpaks; language: Termux's `LANG` |
| Battery, power saver, network | ✅ | battery and battery saver from Android through the helper (every 2 min; `lindroids info`, PowerProfileMonitor); network from the page |
| Low memory | ❌ | |
| Microphone | 🟡 | PulseAudio recording from the page's microphone (desktop Chromium: −21 dBFS from a fake mic); the VM has no microphone to check |
| Screen density, rotation | 🟡 | integer scale fixed at connect; rotation resizes |
| Soft keyboard, hardware keyboard | 🟡 | US layout only outside text-input programs |
| Typed text arrives in order | ✅ | what keys can type goes as keys, not text-input (keys overtook it); keys wait while Firefox holds a composition open for one injected key (`echo` had arrived as `eco…h`); lab test in Firefox (b16) |

## Speed (b16, software rendering)

| Criterion | Status | Evidence / notes |
|---|---|---|
| Tap on the icon to a drawn window | ✅ | ~1–2 s (Calculator) |
| Scrolling | ✅ | 55 fps, 8.5 Mpx/s, 1.4 ms to draw a frame (server started with `LINDROIDS_STATS=1`) |
| Typing and taps feel immediate | ✅ | |

## Demos

Recorded with `lab/record.sh` (the VM has no hardware encoder) in `media/demos/`:
`launch-from-home` (icon → Calculator with Termux closed → close), `open-with` (Files → Termux →
Mousepad), `file-picker` (Text Editor → Android's picker), `running-icon` (status bar icon and
notification bring the window back).
