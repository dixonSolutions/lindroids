# Installing Lindroids

Lindroids has two halves: the **Lindroids app** (an Android APK) and the **`lindroids` CLI**,
which is also the server and runs wherever your Linux programs run: Termux, a Debian/Ubuntu
rootfs under proot (proot-distro, UserLAnd, Andronix), or an ordinary Linux machine.

One command installs or updates the CLI anywhere:

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
```

What it does depends on where it runs:

| Where                                      | How                                                        | Build                     |
| ------------------------------------------ | ---------------------------------------------------------- | ------------------------- |
| Termux (`$PREFIX` contains `com.termux`)   | adds the Lindroids APT repository, suite `termux`          | bionic (Android libc)     |
| Debian, Ubuntu and derivatives, incl. proot | adds the Lindroids APT repository, suite `stable`         | static musl               |
| anything else (Alpine, Arch, Fedora, …)    | downloads the static binary into `~/.local/bin`           | static musl               |

Both APT packages also carry the APK, so the app is updated together with the CLI.

The installer can be read first: `curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | less`.
It honours these variables: `LINDROIDS_METHOD` (`apt-termux`, `apt-debian` or `static`),
`LINDROIDS_INSTALL_DIR` (static installs), `LINDROIDS_VERSION` (static installs; default: latest)
and `LINDROIDS_REPO_URL` (a mirror or a local test repository).

It also checks for Flatpak and, when it is missing, asks whether to install it with Flathub. Termux
has no flatpak package, so there it would go into a proot-distro distro (Debian unless
`LINDROIDS_DISTRO` names another), where `lindroids run <app-id>` finds its apps on its own. That
downloads a whole distro, and Lindroids does not need it for Termux programs, so in Termux the
question defaults to no.
`LINDROIDS_FLATPAK=yes` or `no` answers the question in advance; with no terminal it is skipped.

## Termux

Use Termux from F-Droid or GitHub (the Play Store build works too).

```sh
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
termux-open $PREFIX/share/lindroids/lindroids.apk     # install the app, once
lindroids pair                                        # enter the code in the app
lindroids run <command>
```

Optional packages: `pkg install proot-distro` to run programs from a Debian/Ubuntu/Arch rootfs
(`lindroids run --distro debian …`). For X11 programs, `lindroids setup x11` installs Xwayland
(from Termux's x11-repo); xwayland-satellite, which gives every X11 window its own Android window,
comes with Lindroids. Anywhere else, `lindroids setup x11` uses that system's package manager.

**Update:** `pkg upgrade` (or `apt update && apt install lindroids`). The package always carries
the matching APK; to update the app, open `$PREFIX/share/lindroids/lindroids.apk` again with
`termux-open`.

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

and uninstall the Lindroids app from Android.

## Debian and Ubuntu, including proot-distro, UserLAnd and Andronix

Run as root (the usual case in a proot rootfs) or as a user with `sudo`:

```sh
apt-get update && apt-get install -y curl    # if curl is missing
curl -fsSL https://dixonsolutions.github.io/lindroids/install.sh | sh
lindroids pair
lindroids run <command>
```

The package is a static binary with no dependencies, for `arm64`, `armhf`, `amd64` and `i386`.
The APK is at `/usr/share/lindroids/lindroids.apk`; install it on the phone once (from Termux:
`termux-open` on the same file inside the rootfs, or copy it to shared storage and open it).

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

The app comes from the release too (`lindroids-<version>.apk`).

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
- `apt update` cannot reach the repository from a proot distro: install `ca-certificates`.
- `lindroids doctor` shows what the CLI found (sandbox, proot, Xwayland, xwayland-satellite, server, app, the X display).
