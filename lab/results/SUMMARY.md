# Lindroids lab results

Written by lab/test-lindroids.sh (one row per VM and SELinux mode; re-running replaces the row).

## With the Lindroids app (until 2026-10-26)

| VM | SELinux | Android | Image | Paired | Transport | Window | Typing | Reconnect | AVC denials (app/Termux) | Result | Date |
|----|---------|---------|-------|--------|-----------|--------|--------|-----------|--------------------------|--------|------|
| b16 (app) | Permissive | 13 (sdk 33) | TQ3A.230901.001.C1 | yes | unix | yes | yes | yes 1836ms | 4 | PASS | 2026-10-26 |
| a9 (app) | Permissive | 9 (sdk 28) | android_x86_64-userdebug 9 PI eng.lh.20200325.112926 test-keys | yes | unix | yes | yes | yes 1359ms | 9 | PASS | 2026-10-26 |
| a7 (app) | Permissive | 7.1.2 (sdk 25) | android_x86_64-userdebug 7.1.2 N2G48H eng.cw_hua.20210214.062339 test-keys | yes | unix | yes | yes | yes 795ms | 12 | PASS | 2026-10-26 |
| b16 (app) | Enforcing | 13 (sdk 33) | TQ3A.230901.001.C1 | yes | tcp | yes | yes | yes 1515ms | 6 | PASS | 2026-10-26 |
| a7 (app) | Enforcing | 7.1.2 (sdk 25) | android_x86_64-userdebug 7.1.2 N2G48H eng.cw_hua.20210214.062339 test-keys | yes | unix | yes | yes | yes 1124ms | 0 | PASS | 2026-10-26 |
| a9 (app) | Enforcing | 9 (sdk 28) | android_x86_64-userdebug 9 PI eng.lh.20200325.112926 test-keys | yes | tcp | yes | yes | yes 892ms | 7 | PASS | 2026-10-26 |
| b15 (app) | Permissive | 12 (sdk 32) | SQ3A.220705.004 | yes | unix | yes | yes | yes 929ms | 3 | PASS | 2026-10-26 |
| b15 (app) | Enforcing | 12 (sdk 32) | SQ3A.220705.004 | yes | tcp | yes | yes | yes 942ms | 6 | PASS | 2026-10-26 |

## Windows of their own: Custom Tabs opened by Termux (from 2026-10-27)

New rows go below.

| VM | SELinux | Android | Image | Browser | Linked | Window | Typing | Closing ends it | AVC denials (browser/Termux) | Result | Date |
|----|---------|---------|-------|---------|--------|--------|--------|-----------------|------------------------------|--------|------|
| b16 | Permissive | 13 (sdk 33) | TQ3A.230901.001.C1 | org.mozilla.firefox | yes | yes | yes | yes 5s | 10 | PASS | 2026-10-27 |
| b16 | Enforcing | 13 (sdk 33) | TQ3A.230901.001.C1 | org.mozilla.firefox | yes | yes | yes | yes 5s | 3 | PASS | 2026-10-27 |

## Recents and closing both ways (from 2026-10-28)

| VM | SELinux | Android | Browser | Linked | Window | Recents | Typing | Window closed → program ends | Program ends → window closes | Shortcut | AVC denials (browser/Termux) | Result | Date |
|----|---------|---------|---------|--------|--------|---------|--------|------------------------------|------------------------------|----------|------------------------------|--------|------|
| b16 | Enforcing | 13 (sdk 33) | org.mozilla.firefox (default org.mozilla.firefox) | yes | yes | browser's icon (label "none") | yes | yes 5s | no: Firefox ignores window.close(), the ended screen asks for ✕ | yes 2s | 18 | PASS | 2026-10-28 |
| b16 | Enforcing | 13 (sdk 33) | org.cromite.cromite (default org.mozilla.firefox) | yes | yes | program's: "foot", icon 72x72 (recents-icon.png) | yes | yes 5s | yes 2s | yes 3s | 22 | PASS | 2026-10-28 |
