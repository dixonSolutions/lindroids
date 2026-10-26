#!/usr/bin/env bash
# Download (resumable, checksum-verified) the Android-x86 / BlissOS images of the Lindroids lab and
# extract the kernel and initrd that lab/vm.sh boots directly (-kernel/-initrd).
#
# The ISO itself stays: it is attached as a read-only cdrom and the initrd finds system.sfs/.efs on
# it (cmdline SRC=). Extracting system.sfs and deleting the ISO would save nothing — system.sfs is
# >95% of the ISO — and would need a second disk image to hold it.
#
#   lab/fetch-images.sh            fetch every image
#   lab/fetch-images.sh a9 b15     fetch only these
#   lab/fetch-images.sh --list     show the table
#
# Data lives outside the repo, in $LAB_HOME (default ~/VMs/lindroids-lab). Never /tmp (RAM).
set -euo pipefail

LAB_HOME=${LAB_HOME:-$HOME/VMs/lindroids-lab}
IMAGES=$LAB_HOME/images
SF=https://downloads.sourceforge.net/project

# name | Android | ISO url (or local path) | md5 (SourceForge publishes md5 for every file) | size
TABLE=$(cat <<EOF
a7|7.1 Nougat (Android-x86 7.1-r5)|$SF/android-x86/Release%207.1/android-x86_64-7.1-r5.iso|df88cdebf2cda94c3ec1bfc6d201b11f|921698304
a9|9 Pie (Android-x86 9.0-r2)|$SF/android-x86/Release%209.0/android-x86_64-9.0-r2.iso|e9ba997cede7bf3514d2ba3e624ff767|965738496
b15|12L (BlissOS 15.9.2 foss)|$SF/blissos-x86/Official/BlissOS15/FOSS/Generic/Bliss-v15.9.2-x86_64-OFFICIAL-foss-20241011.iso|7473f738d56f6652c7061d9bcb10f5f6|2019557376
b16|13 (BlissOS 16.9.7 foss)|file://$HOME/Downloads/Bliss-v16.9.7-x86_64-OFFICIAL-foss-20241011.iso|-|-
EOF
)

if [[ ${1:-} == --list ]]; then
  echo "$TABLE" | column -t -s'|'
  exit 0
fi

want=("$@")
mkdir -p "$IMAGES"

fetch_one() {
  local name=$1 url=$2 md5=$3 size=$4
  local dir=$IMAGES/$name iso
  mkdir -p "$dir"
  if [[ $url == file://* ]]; then
    # Reused read-only (the Bliss 16 ISO also backs the lead's VM): link it, never copy or modify it.
    iso=${url#file://}
    [[ -f $iso ]] || { echo "$name: missing $iso" >&2; return 1; }
    ln -sfn "$iso" "$dir/android.iso"
  else
    iso=$dir/android.iso
    if [[ ! -f $iso ]]; then
      echo "$name: downloading $(basename "$url") ($((size / 1048576)) MiB)"
      # -C - resumes a partial .part file; SourceForge redirects to a mirror (-L).
      # Mirrors drop long transfers (curl 56); resume up to 20 times.
      local try
      for try in $(seq 1 20); do
        curl -fL --retry 8 --retry-all-errors --retry-delay 5 --connect-timeout 30 -C - -o "$iso.part" "$url" && break
        echo "$name: transfer interrupted (attempt $try), resuming" >&2
        sleep 5
      done
      local got
      got=$(stat -c %s "$iso.part")
      [[ $size == - || $got == "$size" ]] || { echo "$name: size $got != $size (re-run to resume)" >&2; return 1; }
      if [[ $md5 != - ]]; then
        echo "$name: verifying md5"
        if ! echo "$md5  $iso.part" | md5sum -c --quiet -; then
          echo "$name: md5 mismatch, deleting the download" >&2
          rm -f "$iso.part"
          return 1
        fi
      fi
      mv "$iso.part" "$iso"
    fi
  fi
  # kernel + initrd for -kernel/-initrd; boot menus kept as the reference for each image's options.
  (cd "$dir" && bsdtar -xf "$iso" kernel initrd.img 2>/dev/null || true)
  (cd "$dir" && bsdtar -xf "$iso" boot/grub/grub.cfg isolinux/isolinux.cfg efi/boot/grub.cfg 2>/dev/null || true)
  [[ -s $dir/kernel && -s $dir/initrd.img ]] || { echo "$name: no kernel/initrd.img in the ISO" >&2; return 1; }
  echo "$name: ready ($dir)"
}

# magiskpolicy (from the Magisk APK) patches the live SELinux policy for SELINUX=late runs; see
# lab/selinux-shim.rules. Pinned: sha256 of the extracted x86_64 binary of Magisk v28.1.
MAGISK_URL=https://github.com/topjohnwu/Magisk/releases/download/v28.1/Magisk-v28.1.apk
MAGISKPOLICY_SHA256=aa9dd9e31fb9e01a9f4e43eb2d988b38f9cb5cb84ba58c8e7d0d2aecaa9c60f4
fetch_magiskpolicy() {
  local t=$LAB_HOME/tools
  mkdir -p "$t"
  if [[ -x $t/magiskpolicy ]] && echo "$MAGISKPOLICY_SHA256  $t/magiskpolicy" | sha256sum -c --quiet - 2>/dev/null; then return 0; fi
  echo "tools: fetching magiskpolicy (Magisk v28.1)"
  curl -fL --retry 5 -o "$t/magisk.apk" "$MAGISK_URL"
  bsdtar -xOf "$t/magisk.apk" lib/x86_64/libmagiskpolicy.so >"$t/magiskpolicy"
  rm -f "$t/magisk.apk"
  chmod 755 "$t/magiskpolicy"
  echo "$MAGISKPOLICY_SHA256  $t/magiskpolicy" | sha256sum -c --quiet - || { echo "magiskpolicy checksum mismatch" >&2; rm -f "$t/magiskpolicy"; return 1; }
}

rc=0
fetch_magiskpolicy || rc=1
while IFS='|' read -r name what url md5 size; do
  [[ -z $name ]] && continue
  if ((${#want[@]})) && [[ ! " ${want[*]} " == *" $name "* ]]; then continue; fi
  df -h --output=avail "$LAB_HOME" | tail -1 | sed "s/^/$name: free disk /"
  fetch_one "$name" "$url" "$md5" "$size" || rc=1
done <<<"$TABLE"
du -sh "$LAB_HOME" 2>/dev/null || true
exit $rc
