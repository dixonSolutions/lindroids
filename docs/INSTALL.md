# Installing Lindroids

Lindroids is one Linux package, the **`lindroids` CLI**, which is also the server and runs
wherever your Linux programs run: Termux, a Debian/Ubuntu rootfs under proot (proot-distro,
UserLAnd, Andronix), or an ordinary Linux machine. There is no Android app to install: the
windows show in the phone's own web browser, on a page the server serves at
`http://127.0.0.1:<port>/`, which `lindroids run` opens.

One command installs or updates it anywhere:

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
```

It finds the package manager in use and installs the way that fits it:

| Where                                        | How                                                        | Build                     |
| -------------------------------------------- | ---------------------------------------------------------- | ------------------------- |
| Termux with apt (the default)                | adds the Lindroids APT repository, suite `termux`          | bionic (Android libc)     |
| Termux with pacman (the pacman bootstrap)    | adds the Lindroids pacman repository `pacman/termux`       | bionic (Android libc)     |
| Debian, Ubuntu and derivatives, incl. proot  | adds the Lindroids APT repository, suite `stable`          | static musl               |
| Arch Linux and derivatives, incl. proot      | adds the Lindroids pacman repository `pacman/arch`         | static musl               |
| anything else (Fedora, Alpine, openSUSE, Void, …) | downloads the static binary into `~/.local/bin`       | static musl               |

The installer can be read first: `curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | less`.
It honours these variables: `LINDROIDS_METHOD` (`apt-termux`, `pacman-termux`, `apt-debian`,
`pacman-arch` or `static`),
`LINDROIDS_INSTALL_DIR` (static installs), `LINDROIDS_VERSION` (static installs; default: latest)
and `LINDROIDS_REPO_URL` (a mirror or a local test repository).

It also checks for Flatpak and, when it is missing, asks whether to install it with Flathub, with
the package manager it found: apt, pacman, dnf, yum, zypper, apk or xbps. Termux has no flatpak
package, so there it offers to install Flatpak into a proot-distro distro you already have (the
first one, or `LINDROIDS_DISTRO`), with that distro's own package manager; it never installs
proot-distro or a distro itself, and with no distro it skips Flatpak.
`LINDROIDS_FLATPAK=yes` or `no` answers the question in advance; with no terminal it is skipped.

## Termux

Use Termux from F-Droid or GitHub (the Play Store build works too).

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
lindroids run <command>
```

`lindroids run` opens the windows page in the browser (with `termux-open-url`) and the program's
windows show there.

Optional packages: `pkg install proot-distro` to run programs from a Debian/Ubuntu/Arch rootfs
(`lindroids run --distro debian …`). For X11 programs, `lindroids setup x11` installs Xwayland
(from Termux's x11-repo); xwayland-satellite, which gives every X11 window its own window on the page,
comes with Lindroids. Anywhere else, `lindroids setup x11` uses that system's package manager.

**Pairing:** the page `lindroids run` opens carries the server's secret, so it needs no code. A
page opened by hand (`http://127.0.0.1:<port>/`) asks for the 6-digit code that `lindroids pair`
prints.

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
rm -rf ~/.local/share/lindroids        # pairing secret and exported-app records
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

## Other distributions and plain Linux

The installer downloads `lindroids-<version>-<arch>-linux-musl.tar.gz` from the latest
[GitHub release](https://github.com/dixonSolutions/lindroids/releases/latest), checks it against
the release's `SHA256SUMS`, and installs `~/.local/bin/lindroids` (`LINDROIDS_INSTALL_DIR` to
change). Architectures: `x86_64`, `aarch64`, `armv7`, `i686`. Make sure `~/.local/bin` is in
`PATH`.

The same tarballs can be installed by hand:

```sh
tar -xzf lindroids-<version>-x86_64-linux-musl.tar.gz
install -m 755 lindroids-<version>-x86_64-linux-musl/lindroids ~/.local/bin/
```

**Update:** run the installer again. **Uninstall:** `rm ~/.local/bin/lindroids ~/.local/share/lindroids -r`.

## Verifying the repository key

The repository is signed with an ed25519 key:

```
Lindroids APT repository <lindroids@users.noreply.github.com>
CF71 814E DB63 C911 B430  479C 6FFD 91C9 0F43 BF7B
```

Check a downloaded key with `gpg --show-keys lindroids.gpg`. The same key is in the source tree as
`packaging/repo/lindroids.gpg`.

## Troubleshooting

- `NO_PUBKEY` or `not signed` from apt: the key file is missing or stale; run the installer again.
- `unknown trust` or `invalid or corrupted package (PGP signature)` from pacman: the key is not
  trusted in pacman's keyring; run the installer again (it runs `pacman-key --add` and
  `--lsign-key`).
- `apt update` cannot reach the repository from a proot distro: install `ca-certificates`.
- `lindroids doctor` shows what the CLI found (sandbox, proot, Xwayland, xwayland-satellite, server, display, the X display).
