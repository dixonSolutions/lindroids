---
title: "Installing apps with no taps"
sidebarTitle: "No-tap installs (adb)"
description: "Pair Termux once with Android's Wireless debugging, and `lindroids export` installs a program's app by itself."
---

`lindroids export` builds a program's app on the phone in milliseconds ([A program's own
app](APPS.md)). Installing it normally takes Android's installer and the user's taps: with Termux
from Google Play, which may not open the installer, the app goes to **Download** and the user taps
it in the file manager, then *Install*, sometimes Play Protect's questions too.

ADB is the other way in. The shell user installs apps with no installer at all, and Android 11+
can run adbd on the phone itself, **Wireless debugging**, which pairs with a six-digit code. Termux's
own `adb` (package `android-tools`) pairs with it once, like a computer would, and from then on:

```sh
lindroids export com.vscodium.codium
```

```
Built VSCodium's app (org.lindroids.app.com.vscodium.codium, 181 KB, 24 ms).
Installed through Termux's adb, with no taps: VSCodium's icon is in the app list. ...
```

## Setting it up, once

```sh
lindroids adb pair
```

1. It offers `pkg install android-tools` if Termux has no `adb` (about 7 MB).
2. It opens Developer options. Turn them on first if needed (Settings → About → Software
   information → tap *Build number* 7 times).
3. *Wireless debugging* → *On* (needs Wi-Fi) → *Pair device with pairing code*.
4. Keep the code on screen, with Termux beside Settings (split screen or a pop-up window), and type
   it into Termux: leaving the code's dialog cancels it.

Lindroids finds the pairing port itself, pairs, finds Wireless debugging's own port, checks that it
reaches the phone's shell, and remembers it. `lindroids adb pair <code> <port>` takes both from the
dialog without asking.

| Command | |
|---|---|
| `lindroids adb` | adb installed? paired? Wireless debugging on now, and on which port |
| `lindroids adb pair [<code> [<port>]]` | pair Termux with Wireless debugging |
| `lindroids adb forget` | exports use Android's installer again (forget *Termux* under Wireless debugging → *Paired devices* too) |

## What happens on export

While Wireless debugging is on, `export` (and `lindroids app install`) installs with
`adb install -r` over 127.0.0.1 and disconnects. When it is off, which happens when Wi-Fi goes and
after a restart, the export says so and falls back to Android's installer as before. Its Quick
Settings tile (Developer options → *Quick settings developer tiles* → *Wireless debugging*) turns
it on again; the pairing stays.

The ports are found on the phone only:

- **mDNS**: Android advertises `_adb-tls-pairing._tcp` and `_adb-tls-connect._tcp`; Lindroids asks
  with a unicast-answer query. Other phones on the Wi-Fi may answer, so a port counts only if it
  answers on 127.0.0.1.
- **127.0.0.1's ports**: 5555 and the kernel's range for port 0 (`ip_local_port_range`, normally
  32768–60999), in parallel, each asked with an ADB `CNXN`: Wireless debugging's adbd answers
  `STLS`, `adb tcpip` answers `AUTH`, an adbd without keys answers `CNXN`, and the pairing server
  (TLS) answers with a TLS alert. A scan takes about 5 s; the port found is remembered, so later
  exports ask it first.

`adb tcpip 5555`, done once from a computer, works too: the phone then asks once to allow
Termux's key ("Allow USB debugging?").

## Why ADB

- **No taps**, on every Termux: the Google Play build cannot open Android's installer at all.
- **No Play Protect dialogs** in the way of the installer's (*assumed*; to be checked on the
  tablet: Play may still scan apps installed over ADB, silently).
- **Google's developer verification** (2026–2027) is meant to leave ADB installs alone, while
  installs of apps signed by an unregistered key through the installer will need the user's
  one-time "advanced flow".

What it costs: Developer options on, and Wireless debugging on while exporting. Any program in
Termux can then use Termux's paired key to get the phone's shell (the same as a paired computer).
`lindroids adb forget` and removing *Termux* under *Paired devices* undo it.

## Tested

Lab VM b15 (Bliss OS, Android 12L, x86_64, Termux 0.118 from F-Droid, 2026-11-02). It has no Wi-Fi,
so no Wireless debugging; its adbd listens on 5555 with no keys, which tests the rest:

| What | Result |
|---|---|
| `pkg install android-tools` | adb 37.0.0p1 (`adb mdns` "not supported by this version", hence Lindroids' own mDNS and scan) |
| `adb connect 127.0.0.1:5555` from Termux, `adb install -r` foot's app | `Success` in 0.47 s |
| `lindroids adb` (no port remembered) | finds adbd on 5555 in 5.4 s; with the port remembered: 0.01 s |
| `lindroids adb pair` with adbd reachable | "reaches this phone already", remembered |
| `lindroids export foot` | built in 29 ms, installed (versionCode 29888741 → 29892978), 2.2 s in all, no installer; foot's icon opened foot in its app |
| `lindroids adb forget`, `export foot` | Android's installer opened, as before |

Still to test, on the tablet (Android 16, Termux from Google Play): `android-tools` in Play
Termux's repository; Wireless debugging's adbd on 127.0.0.1; mDNS answers from Termux; pairing in
split screen; Play Protect on ADB installs; Wireless debugging off → the installer fallback.
