# Installing Lindroids

Lindroids is one Linux package, the **`lindroids` CLI**, which is also the server and runs
wherever your Linux programs run: Termux, a Linux distribution under proot (proot-distro,
UserLAnd, Andronix), or an ordinary Linux machine. There is no Android app to install: on Android
every program gets a window of its own, a full-screen Custom Tab of a browser already on the phone
(Firefox, Chrome, Edge, Samsung Internet, Brave…), which `lindroids run` opens from Termux. On a
desktop the windows show in a browser page.

One command installs or updates it anywhere:

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
```

It finds the package manager in use and installs the way that fits it:

| Where                                        | How                                                        | Build                     |
| -------------------------------------------- | ---------------------------------------------------------- | ------------------------- |
| Termux with apt (the default)                | adds the Lindroids APT repository, suite `termux`          | bionic (Android libc)     |
| Termux with pacman (the pacman bootstrap)    | adds the Lindroids pacman repository `pacman/termux`       | bionic (Android libc)     |
| Debian, Ubuntu, Kali, Mint and derivatives   | adds the Lindroids APT repository, suite `stable`          | static musl               |
| Arch, Manjaro, Artix, EndeavourOS            | adds the Lindroids pacman repository `pacman/arch`         | static musl               |
| Fedora, RHEL 8+, Rocky, Alma, CentOS Stream, Oracle, Amazon Linux 2023 | adds the Lindroids RPM repository `rpm/`, installs with dnf | static musl |
| openSUSE Tumbleweed and Leap, SLE            | adds the same RPM repository, installs with zypper         | static musl               |
| Alpine, postmarketOS, Adélie, Chimera        | adds the Lindroids apk repository `apk/`                    | static musl               |
| Void (glibc and musl)                        | adds the Lindroids xbps repository `xbps/`                  | static musl               |
| anything else (Amazon Linux 2, CentOS 7, Gentoo, NixOS, …) | downloads the static binary into `~/.local/bin` | static musl            |

All of them work the same under proot (proot-distro, UserLAnd, Andronix). The installer goes by
what is really there: the package managers that are installed and run, with `/etc/os-release`
(`ID`, `ID_LIKE`) deciding between several (a Debian with pacman installed uses apt). When the
repository has no package for the CPU, or is not reachable, it installs the static binary instead
and says so. Outside Termux it needs root for the system changes: as root (the usual case in a
proot distro) it runs them directly, otherwise with `sudo` (or `doas`); it prints what it is about
to change first.

The installer can be read first: `curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | less`.
It honours these variables: `LINDROIDS_METHOD` (`apt-termux`, `pacman-termux`, `apt-debian`,
`pacman-arch`, `dnf`, `yum`, `zypper`, `apk`, `xbps` or `static`),
`LINDROIDS_INSTALL_DIR` (static installs), `LINDROIDS_VERSION` (static installs; default: latest)
and `LINDROIDS_REPO_URL` (a mirror or a local test repository).

It also checks for Flatpak and, when it is missing, asks whether to install it with Flathub, with
the package manager it found: apt, pacman, dnf, yum, zypper, apk or xbps. Termux has no flatpak
package, so there it shows a menu:

1. **Lindroids' Flatpak for Termux** (the default): ostree and Flatpak built from their release
   sources with Lindroids' Android patches (`packaging/termux-flatpak`), right there on the phone,
   once: about 20 minutes and 600 MB of build tools. No root and no Linux distro: apps install per user,
   `flatpak --user install flathub <app-id>`, and run with `lindroids run <app-id>`. With root
   (`tsu`), `flatpak run` works too, sandboxed by bubblewrap from Termux's root-repo;
2. **install it in a proot-distro distro you have** (one entry per distro), with its own package
   manager;
3. **set up a new Debian proot-distro container** with it;
4. **skip it** — `lindroids setup flatpak` shows the same menu later.

A Flatpak already there (in Termux or in a distro) is used as it is; Lindroids' own is brought up
to date. `LINDROIDS_FLATPAK=native` (the package, else a build), `build` (always from source),
`debian`, `distro:<name>` or `no` answers the menu in advance.
`LINDROIDS_FLATPAK=yes` or `no` answers the questions in advance; with no terminal they are skipped.

## Termux

Use Termux from F-Droid or GitHub (the Play Store build works too).

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh -o install.sh && sh install.sh
lindroids run <command>
```

`lindroids run` opens the windows page in the browser (with `termux-open-url`) and the program's
windows show there.

Optional packages: `pkg install proot-distro` to run programs from a Debian/Ubuntu/Arch rootfs
(`lindroids run --distro debian …`). For X11 programs, `lindroids setup x11` installs Xwayland
(from Termux's x11-repo); xwayland-satellite, which gives every X11 window its own window on the page,
comes with Lindroids. Anywhere else, `lindroids setup x11` uses that system's package manager.

**Pairing:** the window `lindroids run` opens carries a ticket, so the browser pairs by itself
the first time and signs in with its own key after that (`lindroids displays` lists the paired
browsers). A page opened by hand asks for the 6-digit code that `lindroids pair` prints.

Programs installed inside a proot-distro distro (`proot-distro login debian`, then `apt install
xournalpp`) run from Termux as they are: `lindroids run xournalpp` finds that `xournalpp` is not in
Termux but in `debian`, and runs it there, as `lindroids run --distro debian xournalpp` would.

**Update:** `pkg upgrade` (or `apt update && apt install lindroids`), or `lindroids update`.

**Manual setup** (what the installer does):

```sh
mkdir -p $PREFIX/etc/apt/keyrings
curl -fsSL https://dixonsolutions.github.io/lindroids/lindroids.gpg -o $PREFIX/etc/apt/keyrings/lindroids.gpg
echo "deb [signed-by=$PREFIX/etc/apt/keyrings/lindroids.gpg] https://dixonsolutions.github.io/lindroids termux main" \
  > $PREFIX/etc/apt/sources.list.d/lindroids.list
apt update && apt install lindroids
```

**Uninstall:**

```sh
apt remove lindroids
rm $PREFIX/etc/apt/sources.list.d/lindroids.list $PREFIX/etc/apt/keyrings/lindroids.gpg
rm -rf ~/.local/share/lindroids        # relay key, paired browsers, exported-app records
```

### Termux with pacman

With Termux's pacman bootstrap the installer trusts the repository key with `pacman-key` and adds a
`[lindroids]` repository to `$PREFIX/etc/pacman.conf`, whose settings it keeps in
`$PREFIX/etc/pacman.d/lindroids.conf`. The package is the same as the `.deb` (same files). **Update:** `pkg upgrade` (or `pacman -Syu`). By hand:

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/lindroids.asc -o lindroids.asc
pacman-key --add lindroids.asc && pacman-key --lsign-key CF71814EDB63C911B430479C6FFD91C90F43BF7B
printf '\n[lindroids]\nServer = https://dixonsolutions.github.io/lindroids/pacman/termux/$arch\n' \
  >> $PREFIX/etc/pacman.conf
pacman -Sy lindroids
```

**Uninstall:** `pacman -R lindroids`, then remove the `[lindroids]` section from
`$PREFIX/etc/pacman.conf` and `$PREFIX/etc/pacman.d/lindroids.conf`.

## Debian and Ubuntu, including proot-distro, UserLAnd and Andronix

Run as root (the usual case in a proot rootfs) or as a user with `sudo`:

```sh
apt-get update && apt-get install -y curl    # if curl is missing
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
lindroids pair
lindroids run <command>
```

The package is a static binary with no dependencies, for `arm64`, `armhf`, `amd64` and `i386`.

If both Termux and a proot distro have Lindroids, run the server in one place only.

**Update:** `apt update && apt upgrade`, or run the installer again.

**Manual setup:**

```sh
mkdir -p /etc/apt/keyrings
curl -fsSL https://dixonsolutions.github.io/lindroids/lindroids.gpg -o /etc/apt/keyrings/lindroids.gpg
echo "deb [signed-by=/etc/apt/keyrings/lindroids.gpg] https://dixonsolutions.github.io/lindroids stable main" \
  > /etc/apt/sources.list.d/lindroids.list
apt-get update && apt-get install lindroids
```

**Uninstall:**

```sh
apt-get remove lindroids
rm /etc/apt/sources.list.d/lindroids.list /etc/apt/keyrings/lindroids.gpg
rm -rf ~/.local/share/lindroids
```

## Arch Linux and derivatives, including proot-distro

The same as Termux with pacman, in `/etc`: the key goes into pacman's keyring, `[lindroids]` into
`/etc/pacman.conf` (settings in `/etc/pacman.d/lindroids.conf`), and the package is the static CLI
with xwayland-satellite, for `x86_64`, `aarch64` (Arch Linux ARM), `armv7h` and
`i686`. Run as root or with `sudo`. **Update:** `pacman -Syu`, or run the installer again. By hand,
as above with `/etc/pacman.conf` and `.../pacman/arch/$arch`.

## Fedora, RHEL and openSUSE (dnf, zypper), including proot-distro

One RPM repository for all of them, `x86_64`, `aarch64`, `armv7hl` and `i686`: the static CLI
and xwayland-satellite (glibc), with Xwayland, `libxcb` and `libxcb-cursor` as weak dependencies
(Recommends). Its metadata is signed with the repository key below (`repo_gpgcheck=1`) and holds
every package's checksum. The installer writes `/etc/yum.repos.d/lindroids.repo` (zypper:
`/etc/zypp/repos.d/lindroids.repo`) and installs `lindroids`. **Update:** `dnf upgrade` /
`zypper update`, or run the installer again. By hand:

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/rpm/lindroids.repo -o /etc/yum.repos.d/lindroids.repo
dnf install lindroids
# openSUSE:
zypper addrepo https://dixonsolutions.github.io/lindroids/rpm/lindroids.repo
zypper --gpg-auto-import-keys refresh lindroids && zypper install lindroids
```

**Uninstall:** `dnf remove lindroids` (or `zypper remove lindroids`) and the `.repo` file.

yum without dnf (RHEL/CentOS 7, Amazon Linux 2) has a GnuPG too old for the repository's ed25519
key; the installer uses the static binary there.

## Alpine, postmarketOS and Chimera (apk), including proot-distro

The apk repository has the static CLI for `x86_64`, `aarch64`, `armv7` and `x86`, without
xwayland-satellite: these are musl systems, and `lindroids setup x11` installs Alpine's own
`xwayland-satellite` package. Its index is signed with an RSA key (apk does not use OpenPGP),
`lindroids.rsa.pub`, which the installer puts into `/etc/apk/keys/`; the repository goes into
`/etc/apk/repositories` (`/etc/apk/repositories.d/lindroids.list` where that directory exists).
apk-tools 2 and 3 both read it. **Update:** `apk upgrade`, or run the installer again. By hand:

```sh
wget -O /etc/apk/keys/lindroids.rsa.pub https://dixonsolutions.github.io/lindroids/lindroids.rsa.pub
echo https://dixonsolutions.github.io/lindroids/apk >> /etc/apk/repositories
apk update && apk add lindroids
```

**Uninstall:** `apk del lindroids`, then remove the line and the key.

## Void Linux (xbps), including proot-distro

`/etc/xbps.d/lindroids.conf` points at the xbps repository (`x86_64`, `aarch64`, `armv7l`,
`i686` and their `-musl` variants; the glibc ones also carry xwayland-satellite). It is signed with
the same RSA key as the apk repository; xbps asks to trust it on the first sync, and the installer
answers yes for this repository only. **Update:** `xbps-install -Su`, or run the installer again.
Void images have neither curl nor wget: `xbps-install -S curl` first. By hand:

```sh
echo repository=https://dixonsolutions.github.io/lindroids/xbps > /etc/xbps.d/lindroids.conf
xbps-install -S lindroids        # answer y to import the key
```

**Uninstall:** `xbps-remove lindroids` and `rm /etc/xbps.d/lindroids.conf`.

## Other distributions and plain Linux

The installer downloads `lindroids-<version>-<arch>-linux-musl.tar.gz` from the latest
[GitHub release](https://github.com/dixonSolutions/lindroids/releases/latest), checks it against
the release's `SHA256SUMS`, and installs `~/.local/bin/lindroids` (`LINDROIDS_INSTALL_DIR` to
change) and, on glibc systems, `~/.local/libexec/lindroids/xwayland-satellite`. Architectures:
`x86_64`, `aarch64`, `armv7`, `i686`. Make sure `~/.local/bin` is in `PATH`. If `tar` or `gzip`
is missing it installs them with the package manager.

The same tarballs can be installed by hand:

```sh
tar -xzf lindroids-<version>-x86_64-linux-musl.tar.gz
install -m 755 lindroids-<version>-x86_64-linux-musl/bin/lindroids ~/.local/bin/
```

**Update:** run the installer again. **Uninstall:** `rm ~/.local/bin/lindroids ~/.local/share/lindroids -r`.

## Verifying the repository keys

The APT, pacman and RPM repositories are signed with an ed25519 key:

```
Lindroids APT repository <lindroids@users.noreply.github.com>
CF71 814E DB63 C911 B430  479C 6FFD 91C9 0F43 BF7B
```

Check a downloaded key with `gpg --show-keys lindroids.gpg`. The same key is in the source tree as
`packaging/repo/lindroids.gpg`.

The apk and xbps repositories are signed with an RSA key, `lindroids.rsa.pub` on the site and
`packaging/repo/lindroids.rsa.pub` in the source tree.


## Programs as apps of their own (the default)

`lindroids export <program>` builds the program an Android app of its own on the phone (its name
and icon in the app list and Recents, its window in Android's own WebView, no browser) and hands
it to Android's installer. Nothing is downloaded and no build tools are needed: see
[docs/APPS.md](APPS.md).

```sh
lindroids export <program>            # builds its app; tap Install
lindroids config browser app          # the default; explicit if you changed it before
```

How the APK reaches the installer depends on where Termux came from:

- **Termux from F-Droid or GitHub** may open the installer itself: tap *Install* (the first
  time, allow Termux to install apps).
- **Termux from Google Play** may not (it does not declare `REQUEST_INSTALL_PACKAGES`): Lindroids
  puts `<Name>.apk` in Download and opens the file manager there. Tap it, then *Install*. The
  first time, Android asks to allow the file manager to install apps: allow it, go back, tap the
  APK again.
- **Google Play Protect** may ask to scan the new app: *Scan app* sends it to Google; *More details
  → Install without scanning* installs it after your PIN or fingerprint.

The shared window app (`org.lindroids`, `lindroids app install`) is optional: it shows windows of
programs that are not exported. Its design is in [BROWSER_AGNOSTIC_APK.md](../BROWSER_AGNOSTIC_APK.md).

## Troubleshooting

- `NO_PUBKEY` or `not signed` from apt: the key file is missing or stale; run the installer again.
- `unknown trust` or `invalid or corrupted package (PGP signature)` from pacman: the key is not
  trusted in pacman's keyring; run the installer again (it runs `pacman-key --add` and
  `--lsign-key`).
- `apt update` cannot reach the repository from a proot distro: install `ca-certificates`.
- `UNTRUSTED signature` from apk: `/etc/apk/keys/lindroids.rsa.pub` is missing or stale; run the
  installer again.
- `Gpg Keys not imported` / `Signature verification failed` from dnf or zypper: the repository
  key changed; run the installer again (dnf: `dnf clean all` first).
- `lindroids export` said the app is in Download but nothing installs: the installer closes at once
  when the source app may not install apps; allow the file manager in Android's *Install unknown
  apps* settings, then tap the APK again.
- `lindroids doctor` shows what the CLI found (sandbox, proot, Xwayland, xwayland-satellite, server, display, the X display).
