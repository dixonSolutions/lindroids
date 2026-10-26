# Lindroids lab results

Written by lab/test-lindroids.sh (one row per VM and SELinux mode; re-running replaces the row).

| VM | SELinux | Android | Image | Paired | Transport | Window | Typing | Reconnect | AVC denials (app/Termux) | Result | Date |
|----|---------|---------|-------|--------|-----------|--------|--------|-----------|--------------------------|--------|------|
| b16 | Permissive | 13 (sdk 33) | TQ3A.230901.001.C1 | yes | unix | yes | yes | yes 1836ms | 4 | PASS | 2026-10-26 |
| a9 | Permissive | 9 (sdk 28) | android_x86_64-userdebug 9 PI eng.lh.20200325.112926 test-keys | yes | unix | yes | yes | yes 1359ms | 9 | PASS | 2026-10-26 |
| a7 | Permissive | 7.1.2 (sdk 25) | android_x86_64-userdebug 7.1.2 N2G48H eng.cw_hua.20210214.062339 test-keys | yes | unix | yes | yes | yes 795ms | 12 | PASS | 2026-10-26 |
| b16 | Enforcing | 13 (sdk 33) | TQ3A.230901.001.C1 | yes | tcp | yes | yes | yes 1515ms | 6 | PASS | 2026-10-26 |
| a7 | Enforcing | 7.1.2 (sdk 25) | android_x86_64-userdebug 7.1.2 N2G48H eng.cw_hua.20210214.062339 test-keys | yes | unix | yes | yes | yes 1124ms | 0 | PASS | 2026-10-26 |
| a9 | Enforcing | 9 (sdk 28) | android_x86_64-userdebug 9 PI eng.lh.20200325.112926 test-keys | yes | tcp | yes | yes | yes 892ms | 7 | PASS | 2026-10-26 |
| b15 | Permissive | 12 (sdk 32) | SQ3A.220705.004 | yes | unix | yes | yes | yes 929ms | 3 | PASS | 2026-10-26 |
| b15 | Enforcing | 12 (sdk 32) | SQ3A.220705.004 | yes | tcp | yes | yes | yes 942ms | 6 | PASS | 2026-10-26 |
