---
title: "Packaging and releases"
description: "How every package and repository is built, signed and published, and what a release contains."
---

How a Lindroids release is built, signed and published. For users' instructions see
[INSTALL.md](INSTALL.md).

## What a release contains

Every push to `main` (unless the head commit message contains `[skip ci]`) is a release, built by
[`.github/workflows/release.yml`](https://github.com/dixonSolutions/lindroids/blob/main/.github/workflows/release.yml):

| Artifact                                        | Where it goes                            |
| ----------------------------------------------- | ---------------------------------------- |
| `lindroids_<v>_{aarch64,arm,x86_64,i686}.deb`   | APT suite `termux`; GitHub Release       |
| `lindroids_<v>_{arm64,armhf,amd64,i386}.deb`    | APT suite `stable`; GitHub Release       |
| `lindroids-<v>-1-<arch>.pkg.tar.xz` (from the `.deb`s) | pacman repositories `pacman/termux` (aarch64, arm, x86_64, i686) and `pacman/arch` (aarch64, armv7h, x86_64, i686) |
| `lindroids-<v>-1.{aarch64,armv7hl,x86_64,i686}.rpm` | RPM repository `rpm/` (dnf, yum, zypper) |
| `lindroids-<v>-r0.apk` for aarch64, armv7, x86_64, x86 | apk repository `apk/<arch>/` (Alpine, postmarketOS, Chimera) |
| `lindroids-<v>_1.<arch>.xbps` for aarch64, armv7l, x86_64, i686 and the `-musl` ones (not i686) | xbps repository `xbps/` (Void) |
| `lindroids-<v>-<arch>-linux-musl.tar.gz`        | GitHub Release (static installs): `bin/lindroids` + `libexec/lindroids/xwayland-satellite` |
| `lindroids-<v>.apk` (the window app)            | GitHub Release, every release |
| `SHA256SUMS`                                    | GitHub Release                           |
| the repositories + `install.sh`                 | `gh-pages` → https://dixonsolutions.github.io/lindroids/ |

Flatpak for Termux is not shipped as a package: `install.sh` downloads the build kit
(`packaging/termux-flatpak`, published on the site) and builds Flatpak on the phone. Bump `KIT_REV`
in the kit whenever its patches change.

### Two builds of the CLI

- **Termux** gets a binary built against Android's bionic (`cargo ndk -P 24`, targets
  `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`,
  `i686-linux-android`). A static musl binary does not work there: Android's app seccomp filter
  kills processes that use legacy syscalls musl still issues on x86_64, x86 and arm32.
- **Everything else** (every distro under proot, plain Linux) gets a static musl binary
  (`*-unknown-linux-musl`, linked with `rust-lld`, see `.cargo/config.toml`): the same binary in
  the Debian `.deb`, the Arch package, the `.rpm`, the `.apk`, the `.xbps` and the tarball.

The NDK is the only Android tooling the CLI build uses (cargo-ndk): no Android SDK, no sdkmanager,
no Java. CI takes NDK r27c (`27.2.12479018`) from the runner image when it has that version, else
downloads `android-ndk-r27c-linux.zip` from Google, checks its sha256 and caches it. The optional
window app's build is the exception: it uses a JDK and the SDK's `android.jar`, `aapt2`, `d8`,
`zipalign` and `apksigner`.

The display is a web page the server itself serves (embedded in the binary), so the CLI and its
packages are complete on their own. So is the template of the programs' own apps
(`crates/lindroids-apk/template/window-app.apk`, [APPS.md](APPS.md)): it is committed, as
`shortcut.dex` is, and rebuilt with `packaging/build-app-template.sh` (a JDK and the SDK's
`android.jar`, `aapt2`, `d8`, `zipalign`) only when `packaging/webview-app` changes; the phone fills
it in and signs it in Rust. Optionally, a release also carries the shared window app
([BROWSER_AGNOSTIC_APK.md](../BROWSER_AGNOSTIC_APK.md)): a small APK built by
`packaging/build-webview-apk.sh` with `javac`/`d8`/`aapt2`/`apksigner`, signed with
`LINDROIDS_APP_KEYSTORE` (base64 PKCS#12), `LINDROIDS_APP_KEYSTORE_PASS` and
`LINDROIDS_APP_KEY_ALIAS` (default `lindroids`). The release workflow fails if the key is not set,
so a published release always carries the APK; `LINDROIDS_APP_TEST_SIGN=1` builds a throwaway
test-signed one for the lab, which is never published.

### xwayland-satellite (X11)

`lindroids run --x11` runs X11 programs through
[xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) (MPL-2.0), which runs
Xwayland rootless and makes each X11 window its own toplevel, so each one becomes its own window
on the page. The CLI looks for it at `<dir of lindroids>/../libexec/lindroids/xwayland-satellite`,
then on `PATH`. Nobody packages it for Termux, Debian or Ubuntu, so
[`scripts/build-xwayland-satellite.sh`](https://github.com/dixonSolutions/lindroids/blob/main/scripts/build-xwayland-satellite.sh) builds it from
the pinned release tarball (version, URL and sha256 at the top of the script), unmodified, with
default features:

| Output (`target/xwls/out/`)          | Built                                                              | Shipped at                                         |
| ------------------------------------ | ------------------------------------------------------------------ | -------------------------------------------------- |
| `termux-<arch>/xwayland-satellite`   | cargo-ndk, API 24, against a sysroot of Termux's libxcb `.deb`s     | Termux `.deb`: `$PREFIX/libexec/lindroids/`         |
| `debian-<arch>/xwayland-satellite`   | in `debian:bookworm` (podman or docker), multiarch `-dev` packages and cross gcc | Debian `.deb` and `.rpm`, Void glibc `.xbps`: `/usr/libexec/lindroids/`; tarball: `libexec/lindroids/` |
| `LICENSE.xwayland-satellite`, `xwayland-satellite-crates.txt` | from the source and its `Cargo.lock` | `share/doc/lindroids/` and the tarball |

- **Termux** (`aarch64 arm x86_64 i686`): bindgen (xcb-util-cursor-sys), pkg-config and the
  linker are pointed at a per-arch sysroot unpacked from Termux's own `libxcb`, `xcb-util-cursor`
  and dependency `.deb`s, pinned with sha256 (main and x11-repo; packages.termux.dev and its
  mirrors).
  The binary needs only `libc libdl libm libxcb libxcb-cursor`, has RUNPATH
  `/data/data/com.termux/files/usr/lib` (as Termux's own binaries) and 16 KB-aligned segments.
  Two dependency crates (smithay-client-toolkit, wayland-cursor) do not compile for Android as
  published (they fall back to `shm_open`, which bionic lacks); the Termux build uses them with
  the small patches in [`third_party/xwayland-satellite/`](https://github.com/dixonSolutions/lindroids/tree/main/third_party/xwayland-satellite)
  through `--config patch.crates-io...`, so xwayland-satellite's own source stays unchanged.
  Why each patch exists: [third_party/README.md](https://github.com/dixonSolutions/lindroids/blob/main/third_party/README.md#patches-for-android).
- **Debian** (`arm64 armhf amd64 i386`): built on bookworm, so it needs glibc 2.34 or newer
  (Debian 12, Ubuntu 22.04 and later) plus `libxcb1`, `libxcb-cursor0` and `xwayland`, which
  the `.deb` *recommends* (the static CLI must still install without them). The host's Rust
  toolchain is mounted into the container read-only; crates are downloaded inside it.
- **Termux `.deb`s carry the bionic build only**: X11 programs in proot-distro distros reach
  Termux's display, so they need no satellite of their own. It adds about 5 MB installed (1.3 MB
  download) to a `.deb`. Termux recommends `libxcb`,
  `xcb-util-cursor` and `xwayland` (the last two need `pkg install x11-repo`).
- Every output is checked with `readelf` (NEEDED, RUNPATH, interpreter, alignment, glibc
  version); `scripts/build-xwayland-satellite.sh --check` re-runs the checks. Outputs carry a
  stamp of the script and the patches, so a release rebuilds them only when the pin changes; CI
  caches `target/xwls/out` with the same key.

```sh
scripts/build-xwayland-satellite.sh                     # all 8 (or: termux | debian)
scripts/build-xwayland-satellite.sh termux --arches aarch64
scripts/build-xwayland-satellite.sh --update-pins       # TERMUX_PINS lines for today's Termux
```

To move to a new xwayland-satellite release: set `SATELLITE_VERSION`, `SATELLITE_COMMIT` and
`SATELLITE_SHA256` (`curl -fsSL <tag tarball> | sha256sum`; the script checks the commit in the
tarball's header), check the crate versions in `ANDROID_CRATE_PATCHES` against the new
`Cargo.lock` (a patch that no longer applies fails the build), and update the version table in
[third_party/README.md](https://github.com/dixonSolutions/lindroids/blob/main/third_party/README.md#xwayland-satellite).

## Versioning

[`scripts/version.sh`](https://github.com/dixonSolutions/lindroids/blob/main/scripts/version.sh) defines the version for every artifact:

- version `<major>.<minor>.<N>`: major.minor from `[workspace.package] version` in `Cargo.toml`,
  N = `git rev-list --count HEAD` (so CI checks out with `fetch-depth: 0`);
- `.deb` version = the version; git tag `v<version>`;
- `LINDROIDS_BUILD_ID` = the short commit hash (compiled into the binaries).

To start a new series, bump major.minor in `Cargo.toml`; N keeps counting, so versions stay
monotonic for apt and pacman. Anything set in the environment (`LINDROIDS_VERSION`,
`LINDROIDS_BUILD_ID`) overrides the computed value.

`lindroids version` prints `CARGO_PKG_VERSION+LINDROIDS_BUILD_ID` (for example `0.1.0+9a35dc7`),
so it shows the patch number from `Cargo.toml`, not N. The commit hash identifies the build.

## Building locally

Toolchain: Rust stable with the 8 targets above plus `aarch64-unknown-linux-gnu`,
`armv7-unknown-linux-gnueabihf` and `i686-unknown-linux-gnu` (and the host's
`x86_64-unknown-linux-gnu`) for xwayland-satellite, `cargo-ndk` with the Android NDK r27c
(`ANDROID_NDK_HOME`: `android-ndk-r27c-linux.zip` unpacked; default the newest
`~/Android/Sdk/ndk/*`), `dpkg-deb`, `bsdtar`, `gpg`, `openssl`, `git`; for xwayland-satellite also libclang (Fedora `clang-libs`, Debian
`libclang-dev`), `pkg-config`, `patch`, and podman or docker (inside a distrobox the script uses
the host's podman through `distrobox-host-exec`; `LINDROIDS_CONTAINER_ENGINE` overrides). The
rpm, apk and xbps packages and repositories are made with those distributions' own tools
(`rpmbuild`, `createrepo_c`, `abuild-tar`, `apk index`, `abuild-sign`, `xbps-create`,
`xbps-rindex`) in `fedora`, `alpine` and `void` containers ([`packaging/container.sh`](https://github.com/dixonSolutions/lindroids/blob/main/packaging/container.sh)).

```sh
# Everything into dist/
scripts/build-release.sh

# The signed APT repository (uses the key in ~/.local/share/lindroids/gnupg)
packaging/repo/build-repo.sh .scratch/repo --debs dist/debs
```

Pieces can be run on their own: `scripts/build-xwayland-satellite.sh`,
`packaging/build-debs.sh --version <v> [--arches "x86_64 amd64"] [--xwls target/xwls/out]`,
`packaging/build-pacman.sh [--debs dist/debs] [--out dist/pacman]` (needs `bsdtar`),
`packaging/build-rpm.sh`, `packaging/build-apk.sh`, `packaging/build-xbps.sh`
(`--version <v> [--dist dist] [--arches ...]`).

### Testing a repository before publishing

```sh
tests/install-repos.sh                 # all distros, or e.g.: tests/install-repos.sh alpine fedora
```

[`tests/install-repos.sh`](https://github.com/dixonSolutions/lindroids/blob/main/tests/install-repos.sh) makes the x86_64 package of every kind from
one static CLI, a repository signed with throwaway GPG and RSA keys, serves it on
`127.0.0.1:8790`, and runs `curl …/install.sh | sh` twice in containers of Debian, Ubuntu, Kali,
Arch, Fedora, Rocky 9, openSUSE Tumbleweed and Leap 15.6, Alpine (stable and edge), Chimera, Void
(glibc and musl), and Amazon Linux 2 and busybox (which must fall back to the static tarball,
served there too): each must pick the expected method and install a working `lindroids`. CI runs
it on every pull request (logs uploaded as `install-repos-logs`).

In Termux (the Android emulator reaches the host as 10.0.2.2), against a repository from
`packaging/repo/build-repo.sh .scratch/repo` served with `python3 -m http.server 8790`:

```sh
curl -fsSL http://10.0.2.2:8790/install.sh | LINDROIDS_REPO_URL=http://10.0.2.2:8790 sh
```

`tests/host-integration.sh` runs the server, `lindroids debug-app` (a stand-in for the browser
page) and `foot` through `lindroids run`, types into it, and checks the window's frames; CI runs it
on every pull request and before every release. On pull requests CI also builds (or restores and
re-checks) the 8 xwayland-satellite binaries, and runs `tests/install-repos.sh` (which also checks,
in Debian, that the satellite's libraries resolve and Xwayland comes with it).

## The repositories

Static files on GitHub Pages, built by
[`packaging/repo/build-repo.sh`](https://github.com/dixonSolutions/lindroids/blob/main/packaging/repo/build-repo.sh):

```
dists/termux/{Release,InRelease,Release.gpg}
dists/termux/main/binary-{aarch64,arm,x86_64,i686}/Packages{,.gz}
dists/stable/{Release,InRelease,Release.gpg}
dists/stable/main/binary-{arm64,armhf,amd64,i386}/Packages{,.gz}
pool/termux/main/l/lindroids/lindroids_<v>_<arch>.deb
pool/stable/main/l/lindroids/lindroids_<v>_<arch>.deb
pacman/{termux,arch}/<arch>/lindroids.db{,.tar.gz}{,.sig}
pacman/{termux,arch}/<arch>/lindroids-<v>-1-<arch>.pkg.tar.xz{,.sig}
rpm/lindroids-<v>-1.<arch>.rpm   rpm/repodata/repomd.xml{,.asc,.key} (+ its files)   rpm/lindroids.repo
apk/{aarch64,armv7,x86_64,x86}/APKINDEX.tar.gz   apk/<arch>/lindroids-<v>-r0.apk
xbps/<arch>-repodata   xbps/lindroids-<v>_1.<arch>.xbps{,.sig2}
lindroids.gpg   lindroids.asc   lindroids.rsa.pub   install.sh   VERSION   .nojekyll
index.html   _app/   media/   robots.txt          the website (below)
```

The website at the same address is [`site/`](https://github.com/dixonSolutions/lindroids/tree/main/site): SvelteKit with adapter-static,
Tailwind and shadcn-svelte, theme following the device (mode-watcher), showing `media/` (copied
by `site/copy-media.js`) and the latest `VERSION`. [`site.yml`](https://github.com/dixonSolutions/lindroids/blob/main/.github/workflows/site.yml)
builds it only when `site/`, `media/` or the workflow change, and pull requests only build it.
It shares the release's concurrency group, starts from the published branch and replaces only
the website's files; the release keeps them, and `build-repo.sh` writes a plain `index.html`
only into a repository that has none. Locally: `cd site && npm ci && npm run dev`.

- **RPM** ([`packaging/build-rpm.sh`](https://github.com/dixonSolutions/lindroids/blob/main/packaging/build-rpm.sh), rpmbuild in `fedora`): one
  repository for dnf, yum and zypper, every architecture in one `repodata/` (createrepo_c, gzip
  so older zypper and yum read it). v4 packages with an xz payload, `AutoReqProv: no` (the CLI is
  static), and the satellite's needs as Recommends by file and soname (`/usr/bin/Xwayland`,
  `libxcb.so.1`, `libxcb-cursor.so.0`) so Fedora's and openSUSE's package names both match.
  `repomd.xml` is signed with the APT key (`repomd.xml.asc`; the key also as `repomd.xml.key`);
  clients check it (`repo_gpgcheck=1`) and the packages through the checksums it signs
  (`gpgcheck=0`: the packages themselves are unsigned; the signed metadata covers them). yum 3
  (RHEL/CentOS 7, Amazon Linux 2) has GnuPG 2.0, which cannot check ed25519 signatures, so
  install.sh uses the static tarball there.
- **apk** ([`packaging/build-apk.sh`](https://github.com/dixonSolutions/lindroids/blob/main/packaging/build-apk.sh), abuild-tar in `alpine`): v2
  packages (apk-tools 2 and 3, Alpine, postmarketOS, Adélie, Chimera), made as abuild makes
  them; static CLI only. `APKINDEX.tar.gz` per architecture is signed with the RSA key by
  `abuild-sign` (`.SIGN.RSA.lindroids.rsa.pub`, so clients need `/etc/apk/keys/lindroids.rsa.pub`)
  and checked with `apk verify` against `lindroids.rsa.pub` alone. Packages are unsigned: apk
  checks them against the checksum in the signed index.
- **xbps** ([`packaging/build-xbps.sh`](https://github.com/dixonSolutions/lindroids/blob/main/packaging/build-xbps.sh), xbps-create in `void`): the
  glibc architectures carry the satellite, the `-musl` ones not. `xbps-rindex --sign` signs every
  `<arch>-repodata`, `--sign-pkg` every package (`.sig2`), with the same RSA key.
- Without the RSA key (`LINDROIDS_APK_RSA_KEY`) the apk and xbps repositories are left as they
  are and the release says so; install.sh falls back to the static tarball while a repository is
  not published.

- The pacman packages are made from the `.deb`s by
  [`packaging/build-pacman.sh`](https://github.com/dixonSolutions/lindroids/blob/main/packaging/build-pacman.sh) (same files; `.PKGINFO` and `.MTREE`
  as makepkg writes them; the Termux ones keep the `data/data/com.termux/files/usr` paths, as
  Termux's own pacman packages do). `build-repo.sh` signs every package and the database with the
  same key (binary detached signatures; `%PGPSIG%` in the database) and keeps the newest 3 per
  architecture. `lindroids.db` is a copy of `lindroids.db.tar.gz`: GitHub Pages serves no
  symlinks. install.sh trusts the key with `pacman-key --add`/`--lsign-key`; the fingerprint it
  expects is stamped into it by `build-repo.sh` (`DEFAULT_KEY_FPR`).

- `Release` carries MD5Sum/SHA1/SHA256 for every index; `InRelease` (clearsigned) and
  `Release.gpg` (detached) are signed with SHA-512. No `Valid-Until`, so the repository does not
  expire when no release happens for a while.
- The workflow copies the current `gh-pages` content into `site/` first; the script adds the new
  `.deb`s and keeps the newest 3 versions per architecture (`--keep`). `gh-pages` is pushed as a
  single orphan commit, so old packages do not accumulate in git history.
- `install.sh` is `install.sh` with the published URL and `owner/repo` substituted
  (`--base-url`, `--github-repo`). `VERSION` is the newest version in the pool; the static
  installer downloads that release's tarball.
- The script refuses to sign with any key other than the one committed as
  `packaging/repo/lindroids.gpg`, and verifies both signatures with that file alone, as apt will.
- Termux's architecture `x86_64` makes stock `dpkg-deb` warn ("not a valid architecture name");
  Termux's dpkg accepts it, and the scripts filter the warning.

## Keys and secrets

| What                       | Where (maintainer machine)                         | GitHub secret               |
| -------------------------- | -------------------------------------------------- | --------------------------- |
| APT, pacman and RPM signing key (ed25519) | GNUPGHOME `~/.local/share/lindroids/gnupg` | `LINDROIDS_GPG_PRIVATE_KEY` |
| apk and xbps signing key (RSA 4096, PEM) | `~/.local/share/lindroids/lindroids.rsa`; public half committed as `packaging/repo/lindroids.rsa.pub` | `LINDROIDS_APK_RSA_KEY` |

APT key: `Lindroids APT repository <lindroids@users.noreply.github.com>`, fingerprint
`CF71814EDB63C911B430479C6FFD91C90F43BF7B`, no expiry, no passphrase (it lives in a CI secret).
A revocation certificate is in `~/.local/share/lindroids/gnupg/openpgp-revocs.d/`. Back up
`~/.local/share/lindroids/gnupg` offline: losing the key means every user has to fetch a new key.

[`scripts/set-github-secrets.sh`](https://github.com/dixonSolutions/lindroids/blob/main/scripts/set-github-secrets.sh) sets both secrets with
`gh secret set` (nothing is printed). The first time, it generates the RSA key and writes
`packaging/repo/lindroids.rsa.pub`, which must be committed: `build-repo.sh` refuses an RSA key
that does not match it, and publishes it as `lindroids.rsa.pub`. Back up `lindroids.rsa` with the
GnuPG directory.

### Rotating the APT key

1. Generate a new key in a fresh GNUPGHOME, export it over `packaging/repo/lindroids.gpg` and
   `.asc`, update the fingerprint here and in INSTALL.md, and run `scripts/set-github-secrets.sh`.
2. Commit; the next release is signed with the new key. Users re-run the installer (it rewrites
   the key file). If the old key was compromised, publish its revocation certificate.

### Rotating the RSA key

Move `~/.local/share/lindroids/lindroids.rsa` and `packaging/repo/lindroids.rsa.pub` away, run
`scripts/set-github-secrets.sh` (it makes a new pair and sets the secret), commit the new
`lindroids.rsa.pub`. Alpine and Void users re-run the installer (it installs the new key; xbps
asks to trust it).

### Publishing without the secret

[`scripts/publish-local.sh`](https://github.com/dixonSolutions/lindroids/blob/main/scripts/publish-local.sh) makes the same release from the machine
that holds the key: it builds and signs everything locally, creates the GitHub release, pushes
`gh-pages` and turns Pages on. The key stays on that machine. Without the secret the release
workflow still builds and tests every push, and says it did not publish.

## One-time repository setup

1. Create `github.com/dixonSolutions/lindroids` and push `main`.
2. `scripts/set-github-secrets.sh --release dixonSolutions/lindroids`: sets the secret, runs the
   release workflow, waits for it and turns Pages on (step 4 below).
3. Settings → Actions → General → Workflow permissions: "Read and write" (the workflow pushes
   `gh-pages` and creates releases; it also declares `contents: write`).
4. After the first successful release: Settings → Pages → Deploy from branch `gh-pages`, folder
   `/` (root). No custom domain (no CNAME).

Using a different owner or repository name needs no code change: the workflow derives the Pages
URL and release links from `github.repository`. Only the defaults in `install.sh` and the
URLs in the docs mention `dixonSolutions/lindroids`.
