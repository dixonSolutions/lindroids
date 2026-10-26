# Packaging and releases

How a Lindroids release is built, signed and published. For users' instructions see
[INSTALL.md](INSTALL.md).

## What a release contains

Every push to `main` (unless the head commit message contains `[skip ci]`) is a release, built by
[`.github/workflows/release.yml`](../.github/workflows/release.yml):

| Artifact                                        | Where it goes                            |
| ----------------------------------------------- | ---------------------------------------- |
| `lindroids-<v>.apk` (4 ABIs, release-signed)    | GitHub Release; inside every `.deb`      |
| `lindroids_<v>_{aarch64,arm,x86_64,i686}.deb`   | APT suite `termux`; GitHub Release       |
| `lindroids_<v>_{arm64,armhf,amd64,i386}.deb`    | APT suite `stable`; GitHub Release       |
| `lindroids-<v>-<arch>-linux-musl.tar.gz`        | GitHub Release (static installs): `bin/lindroids` + `libexec/lindroids/xwayland-satellite` |
| `SHA256SUMS`                                    | GitHub Release                           |
| APT repository + `install.sh`                   | `gh-pages` → https://dixonsolutions.github.io/lindroids/ |

### Two builds of the CLI

- **Termux** gets a binary built against Android's bionic (`cargo ndk -P 24`, targets
  `aarch64-linux-android`, `armv7-linux-androideabi`, `x86_64-linux-android`,
  `i686-linux-android`). A static musl binary does not work there: Android's app seccomp filter
  kills processes that use legacy syscalls musl still issues on x86_64, x86 and arm32.
- **Everything else** (glibc distros under proot, plain Linux) gets a static musl binary
  (`*-unknown-linux-musl`, linked with `rust-lld`, see `.cargo/config.toml`).

### proot inside the APK

Apps exported with `lindroids export --bundle` run with Termux closed, under a proot that ships
in the APK. [`scripts/fetch-proot.sh`](../scripts/fetch-proot.sh) downloads Termux's `proot`,
`libtalloc` and `libandroid-shmem` packages at pinned versions (sha256-checked), and installs
`libproot.so`, `libproot-loader.so`, `libproot-loader32.so` (64-bit ABIs), `libtalloc.so` and
`libandroid-shmem.so` into `third_party/proot/<abi>/`, with `patchelf` removing the Termux
RUNPATH and renaming `libtalloc.so.2` to `libtalloc.so`. It then checks with `readelf` that each
library needs only Android system libraries and the files shipped next to it.
`scripts/build-apk.sh` copies those files into `lib/<abi>/`. Licences and sources:
[third_party/README.md](../third_party/README.md).

Termux deletes superseded `.deb`s from its pool, so the pins eventually stop downloading
(the script tries packages.termux.dev, packages-cf.termux.dev and grimler.se in turn, and CI
caches the downloads). To move to the current versions:

```sh
scripts/fetch-proot.sh --update-pins     # prints the PINS lines; paste them into the script
scripts/fetch-proot.sh --force
```

### xwayland-satellite (X11)

`lindroids run --x11` runs X11 programs through
[xwayland-satellite](https://github.com/Supreeeme/xwayland-satellite) (MPL-2.0), which runs
Xwayland rootless and makes each X11 window its own toplevel, so each one becomes an Android
window. The CLI looks for it at `<dir of lindroids>/../libexec/lindroids/xwayland-satellite`,
then on `PATH`. Nobody packages it for Termux, Debian or Ubuntu, so
[`scripts/build-xwayland-satellite.sh`](../scripts/build-xwayland-satellite.sh) builds it from
the pinned release tarball (version, URL and sha256 at the top of the script), unmodified, with
default features:

| Output (`target/xwls/out/`)          | Built                                                              | Shipped at                                         |
| ------------------------------------ | ------------------------------------------------------------------ | -------------------------------------------------- |
| `termux-<arch>/xwayland-satellite`   | cargo-ndk, API 24, against a sysroot of Termux's libxcb `.deb`s     | Termux `.deb`: `$PREFIX/libexec/lindroids/`         |
| `debian-<arch>/xwayland-satellite`   | in `debian:bookworm` (podman or docker), multiarch `-dev` packages and cross gcc | Debian `.deb`: `/usr/libexec/lindroids/`; Termux `.deb` of the same CPU: `$PREFIX/libexec/lindroids/glibc/`; tarball: `libexec/lindroids/` |
| `LICENSE.xwayland-satellite`, `xwayland-satellite-crates.txt` | from the source and its `Cargo.lock` | `share/doc/lindroids/` and the tarball |

- **Termux** (`aarch64 arm x86_64 i686`): bindgen (xcb-util-cursor-sys), pkg-config and the
  linker are pointed at a per-arch sysroot unpacked from Termux's own `libxcb`, `xcb-util-cursor`
  and dependency `.deb`s, pinned with sha256 like proot's (main and x11-repo, same mirrors).
  The binary needs only `libc libdl libm libxcb libxcb-cursor`, has RUNPATH
  `/data/data/com.termux/files/usr/lib` (as Termux's own binaries) and 16 KB-aligned segments.
  Two dependency crates (smithay-client-toolkit, wayland-cursor) do not compile for Android as
  published (they fall back to `shm_open`, which bionic lacks); the Termux build uses them with
  the small patches in [`third_party/xwayland-satellite/`](../third_party/xwayland-satellite/)
  through `--config patch.crates-io...`, so xwayland-satellite's own source stays unchanged.
  Why each patch exists: [third_party/README.md](../third_party/README.md#patches-for-android).
- **Debian** (`arm64 armhf amd64 i386`): built on bookworm, so it needs glibc 2.34 or newer
  (Debian 12, Ubuntu 22.04 and later) plus `libxcb1`, `libxcb-cursor0` and `xwayland`, which
  the `.deb` *recommends* (the static CLI must still install without them). The host's Rust
  toolchain is mounted into the container read-only; crates are downloaded inside it.
- **Termux `.deb`s carry both builds**: the glibc one is for `lindroids export --distro`, which
  copies it into bundles of Debian-family proot distros, which cannot run the bionic one. Together
  they add about 10 MB installed (2 MB to the download) to a Termux `.deb`, and about 5 MB
  installed (1.3 MB download) to a Debian `.deb`. Termux recommends `libxcb`,
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
[third_party/README.md](../third_party/README.md#xwayland-satellite).

## Versioning

[`scripts/version.sh`](../scripts/version.sh) defines the version for every artifact:

- version `<major>.<minor>.<N>`: major.minor from `[workspace.package] version` in `Cargo.toml`,
  N = `git rev-list --count HEAD` (so CI checks out with `fetch-depth: 0`);
- APK `versionCode` = N, `versionName` = the version;
- `.deb` version = the version; git tag `v<version>`;
- `LINDROIDS_BUILD_ID` = the short commit hash (compiled into the binaries).

To start a new series, bump major.minor in `Cargo.toml`; N keeps counting, so versions stay
monotonic for apt and Android. Anything set in the environment (`LINDROIDS_VERSION`,
`LINDROIDS_VERSION_CODE`, `LINDROIDS_BUILD_ID`) overrides the computed value.

`lindroids version` prints `CARGO_PKG_VERSION+LINDROIDS_BUILD_ID` (for example `0.1.0+9a35dc7`),
so it shows the patch number from `Cargo.toml`, not N. The commit hash identifies the build.

## Building locally

Toolchain: Rust stable with the 8 targets above plus `aarch64-unknown-linux-gnu`,
`armv7-unknown-linux-gnueabihf` and `i686-unknown-linux-gnu` (and the host's
`x86_64-unknown-linux-gnu`) for xwayland-satellite, `cargo-ndk`, Android SDK
(`build-tools;35.0.0`, `platforms;android-35`, `ndk;27.2.12479018`), JDK 17+, `patchelf`,
`dpkg-deb`, `gpg`, `git`; for xwayland-satellite also libclang (Fedora `clang-libs`, Debian
`libclang-dev`), `pkg-config`, `patch`, and podman or docker (inside a distrobox the script uses
the host's podman through `distrobox-host-exec`; `LINDROIDS_CONTAINER_ENGINE` overrides).

```sh
# Everything into dist/ (debug-signed APK unless the keystore variables are set)
LINDROIDS_KEYSTORE=~/.local/share/lindroids/release.keystore \
LINDROIDS_KEYSTORE_PASS="$(cat ~/.local/share/lindroids/release.pass)" \
LINDROIDS_KEY_ALIAS=lindroids \
scripts/build-release.sh

# The signed APT repository (uses the key in ~/.local/share/lindroids/gnupg)
packaging/repo/build-repo.sh .scratch/repo --debs dist/debs
```

Pieces can be run on their own: `scripts/fetch-proot.sh`, `scripts/build-apk.sh`,
`scripts/build-xwayland-satellite.sh`,
`packaging/build-debs.sh --version <v> [--arches "x86_64 amd64"] [--xwls target/xwls/out]`.

### Testing a repository before publishing

```sh
packaging/repo/build-repo.sh .scratch/repo --debs dist/debs
python3 -m http.server 8790 --bind 0.0.0.0 --directory .scratch/repo &

# Debian/Ubuntu
podman run --rm --network host docker.io/library/debian:stable sh -ec '
  apt-get update -q && apt-get install -y -q curl
  curl -fsSL http://127.0.0.1:8790/install.sh | LINDROIDS_REPO_URL=http://127.0.0.1:8790 sh
  lindroids version'

# Termux in the Android emulator (the host is 10.0.2.2 there)
curl -fsSL http://10.0.2.2:8790/install.sh | LINDROIDS_REPO_URL=http://10.0.2.2:8790 sh
```

`tests/host-integration.sh` runs the server, `lindroids debug-app` (a stand-in for the Android
app) and `foot` through `lindroids run`, types into it, and checks the window's frames; CI runs it
on every pull request and before every release. On pull requests CI also builds (or restores and
re-checks) the 8 xwayland-satellite binaries, and the package smoke test installs the amd64
`.deb` in `debian:stable` and checks that the satellite's libraries resolve and Xwayland comes
with it.

## The APT repository

Static files on GitHub Pages, built by
[`packaging/repo/build-repo.sh`](../packaging/repo/build-repo.sh):

```
dists/termux/{Release,InRelease,Release.gpg}
dists/termux/main/binary-{aarch64,arm,x86_64,i686}/Packages{,.gz}
dists/stable/{Release,InRelease,Release.gpg}
dists/stable/main/binary-{arm64,armhf,amd64,i386}/Packages{,.gz}
pool/termux/main/l/lindroids/lindroids_<v>_<arch>.deb
pool/stable/main/l/lindroids/lindroids_<v>_<arch>.deb
lindroids.gpg   lindroids.asc   install.sh   VERSION   index.html   .nojekyll
```

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
| APK signing keystore       | `~/.local/share/lindroids/release.keystore`        | `LINDROIDS_KEYSTORE_B64`    |
| keystore password          | `~/.local/share/lindroids/release.pass`            | `LINDROIDS_KEYSTORE_PASS`   |
| key alias (`lindroids`)    |                                                    | `LINDROIDS_KEY_ALIAS`       |
| APT signing key (ed25519)  | GNUPGHOME `~/.local/share/lindroids/gnupg`         | `LINDROIDS_GPG_PRIVATE_KEY` |

APT key: `Lindroids APT repository <lindroids@users.noreply.github.com>`, fingerprint
`CF71814EDB63C911B430479C6FFD91C90F43BF7B`, no expiry, no passphrase (it lives in a CI secret).
A revocation certificate is in `~/.local/share/lindroids/gnupg/openpgp-revocs.d/`. Back up
`~/.local/share/lindroids/` offline: losing the keystore means the app can no longer be updated
in place, and losing the GPG key means every user has to fetch a new key.

[`scripts/set-github-secrets.sh`](../scripts/set-github-secrets.sh) sets all four secrets with
`gh secret set` (nothing is printed or written to disk).

### Rotating the APT key

1. Generate a new key in a fresh GNUPGHOME, export it over `packaging/repo/lindroids.gpg` and
   `.asc`, update the fingerprint here and in INSTALL.md, and run `scripts/set-github-secrets.sh`.
2. Commit; the next release is signed with the new key. Users re-run the installer (it rewrites
   the key file). If the old key was compromised, publish its revocation certificate.

### Publishing without the secrets

[`scripts/publish-local.sh`](../scripts/publish-local.sh) makes the same release from the machine
that holds the keys: it builds and signs everything locally, creates the GitHub release, pushes
`gh-pages` and turns Pages on. The keys stay on that machine. Without the secrets the release
workflow still builds and tests every push, and says it did not publish.

## One-time repository setup

1. Create `github.com/dixonSolutions/lindroids` and push `main`.
2. `scripts/set-github-secrets.sh --release dixonSolutions/lindroids`: sets the secrets, runs the
   release workflow, waits for it and turns Pages on (step 4 below).
3. Settings → Actions → General → Workflow permissions: "Read and write" (the workflow pushes
   `gh-pages` and creates releases; it also declares `contents: write`).
4. After the first successful release: Settings → Pages → Deploy from branch `gh-pages`, folder
   `/` (root). No custom domain (no CNAME).

Using a different owner or repository name needs no code change: the workflow derives the Pages
URL and release links from `github.repository`. Only the defaults in `install.sh` and the
URLs in the docs mention `dixonSolutions/lindroids`.
