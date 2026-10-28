# shellcheck shell=bash
# Sourced by the packaging scripts that need another distribution's tools (apk, rpmbuild,
# createrepo_c, xbps): they run them in that distribution's container image.
#
#   in_container <image> <dir>... -- <command> [args...]
#
# Each <dir> is mounted at the same path, read-write; the command runs as root in the container,
# in the first <dir>. Files it creates in the <dir>s belong to the calling user
# afterwards (rootless podman maps root to that user already; with docker they are chowned).
#
# Engine: LINDROIDS_CONTAINER_ENGINE, else podman, else docker, else (inside a distrobox) the host's
# podman, as scripts/build-xwayland-satellite.sh.

container_engine() {
  if [ -n "${LINDROIDS_CONTAINER_ENGINE:-}" ]; then
    echo "$LINDROIDS_CONTAINER_ENGINE"
  elif command -v podman >/dev/null; then
    echo podman
  elif command -v docker >/dev/null; then
    echo docker
  elif command -v distrobox-host-exec >/dev/null && distrobox-host-exec sh -c 'command -v podman' >/dev/null 2>&1; then
    echo "distrobox-host-exec podman"
  else
    echo "packaging: podman or docker is required" >&2
    return 1
  fi
}

in_container() {
  local image=$1 engine chown_to='' d
  shift
  local -a mounts=() dirs=()
  while [ $# -gt 0 ] && [ "$1" != -- ]; do
    d=$(cd "$1" && pwd)
    mounts+=(-v "$d:$d")
    dirs+=("$d")
    shift
  done
  [ "${1:-}" = -- ] && shift
  engine=$(container_engine) || return 1
  case "$engine" in *docker*) chown_to="$(id -u):$(id -g)" ;; esac
  # SOURCE_DATE_EPOCH with its value: the host's podman (from a distrobox) does not see ours.
  # shellcheck disable=SC2086,SC2016
  $engine run --rm --network host --security-opt label=disable "${mounts[@]}" -w "${dirs[0]}" \
    -e CHOWN_TO="$chown_to" -e SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-}" "$image" \
    sh -c '
      n=$1; shift
      dirs=""
      while [ "$n" -gt 0 ]; do dirs="$dirs $1"; shift; n=$((n - 1)); done
      "$@"; rc=$?
      [ -z "$CHOWN_TO" ] || chown -R "$CHOWN_TO" $dirs
      exit $rc' sh "${#dirs[@]}" "${dirs[@]}" "$@"
}
