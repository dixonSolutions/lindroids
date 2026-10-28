#!/usr/bin/env bash
# The picker's Linux side: the page's `/ls` listing (the source sheet's Linux environment and Home
# options both use it) and its token. The overlay itself is drawn in the page; this checks what it
# gets: one directory at a time, folders first, hidden files marked, and a save allowed only where
# the server can write. The UI was checked by hand on b16 (source sheet, Home > downloads > a file
# opened in VSCodium with its path, Android files opening DocumentsUI); see lab/CHECKLIST.md.
#
#   lab/test-linux-picker.sh [vm]
#
# Uses the lindroids that is on the VM now. Results: lab/results/<vm>/linux-picker/.
set -uo pipefail

LAB_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
REPO=$(cd "$LAB_DIR/.." && pwd)
VM=$LAB_DIR/vm.sh
NAME=${1:-b16}
OUT=$REPO/lab/results/$NAME/linux-picker
rm -rf "$OUT"
mkdir -p "$OUT"
RT='$TMPDIR/lindroids-$(id -u)'
DIR=.lindroids-picker-test
FILE=picker.txt
tsh() { timeout "${TSH_TIMEOUT:-60}" "$VM" "$NAME" ssh "$@"; }
log() { echo "[$(date +%T)] $NAME: $*"; }

tsh 'lindroids start >/dev/null 2>&1; true'
tsh "rm -rf ~/$DIR; mkdir -p ~/$DIR/sub; printf 'picker\\n' > ~/$DIR/$FILE; chmod 555 ~/$DIR/sub; true"
cat >"$OUT/check.sh" <<'EOS'
#!/data/data/com.termux/files/usr/bin/bash
set -u
o=$(cat ~/.local/share/lindroids/origin)
url="http://$o.localhost:47282"
tok=$(curl -fsS -H "Host: $o.localhost:47282" "$url/lindroids.js" | sed -n 's/^const PICKER_TOKEN = .\([^;]*\).*/\1/p' | tr -d "'\"")
ls() { curl -fsS -H "Host: $o.localhost:47282" "$url/ls?t=$tok&path=$1"; }
echo "token=${#tok}"
echo "root=$(ls root)"
echo "home=$(ls ~)"
echo "test=$(ls "$HOME/.lindroids-picker-test")"
echo -n "wrong="
curl -sS -o /dev/null -w '%{http_code}\n' -H "Host: $o.localhost:47282" "$url/ls?t=wrong&path=root"
EOS
"$VM" "$NAME" scp "$OUT/check.sh" .lindroids-picker-check.sh >/dev/null 2>&1
tsh 'sh ~/.lindroids-picker-check.sh' >"$OUT/listing.txt" 2>&1
tsh "rm -f ~/.lindroids-picker-check.sh; rm -rf ~/$DIR; true"

log "checking the /ls answer"
python3 - "$OUT/listing.txt" "$FILE" <<'PY' >"$OUT/result.txt"
import json, re, sys
text = open(sys.argv[1]).read()
name = sys.argv[2]
def field(k):
    m = re.search(rf"^{k}=(.*)$", text, re.M)
    return m.group(1) if m else ""
def obj(k):
    try:
        return json.loads(field(k))
    except Exception:
        return {}
root, home, test = obj("root"), obj("home"), obj("test")
tok = field("token")
ok = {
    "token": tok.isdigit() and int(tok) >= 32,
    "root": root.get("path", "").endswith("/usr") or root.get("path") == "/",
    "home": home.get("path", "") != "",
    "test_folder_first": test.get("entries", [{}])[0].get("name") == "sub",
    "test_file": any(e.get("name") == name and e.get("size") == 7 for e in test.get("entries", [])),
    "sub_read_only": any(e.get("name") == "sub" and e.get("dir") and not e.get("writable") for e in test.get("entries", [])),
    "wrong_token": field("wrong").strip() == "403",
}
for k, v in ok.items():
    print(f"{k}={'yes' if v else 'no'}")
print(f"root_path={root.get('path')}")
print(f"home_path={home.get('path')}")
sys.exit(0 if all(ok.values()) else 1)
PY
cat "$OUT/result.txt"
grep -q '=no$' "$OUT/result.txt" && exit 1 || exit 0
