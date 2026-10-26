# Lindroids lab results

Written by lab/test-lindroids.sh (one row per VM and SELinux mode; re-running replaces the row).

| VM | SELinux | Android | Image | Paired | Transport | Window | Typing | Reconnect | AVC denials (app/Termux) | Result | Date |
|----|---------|---------|-------|--------|-----------|--------|--------|-----------|--------------------------|--------|------|
| b16 | Permissive | 13 (sdk 33) | TQ3A.230901.001.C1 | yes | unix | yes | yes | yes 1836ms | 4 | PASS | 2026-10-26 |
| a9 | Permissive | 9 (sdk 28) | android_x86_64-userdebug 9 PI eng.lh.20200325.112926 test-keys | yes | unix | yes | yes | yes 1359ms | 0 | PASS | 2026-10-26 |
