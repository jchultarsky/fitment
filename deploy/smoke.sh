#!/usr/bin/env bash
# VT29 (docs/research/m6-api.md; OP8, OP11, M6-43): the image as shipped.
# CI's `container` job runs it on every PR after building the image:
#
#   docker build -f deploy/Containerfile -t fitment-server:ci .
#   deploy/smoke.sh fitment-server:ci
#
# 1. The image holds no Python, uv or Open CASCADE (a scan of `docker
#    export`'s file list, which needs no shell in the image), and no shared
#    library beyond its distroless base's.
# 2. Its user is not root; both binaries print the workspace's version.
# 3. A smoke run with two volumes and a read-only root: the CLI makes the
#    catalog, the server starts, health and readiness answer 204 (from the
#    host and through the image's own `healthcheck`), `fitment.info` names
#    the version, and SIGTERM stops it with exit 0. PR 6 adds the bootstrap,
#    an upload of s01 and the committed bracket, and a socket and a match
#    job whose document equals crates/fitment/tests/golden/match/'s.
# 4. Its size is reported, under a ceiling of twice the first measured.
set -euo pipefail
cd "$(dirname "$0")/.."
image=${1:-fitment-server:ci}
# The first measured (arm64, 2026-10-17): 15,041,033 bytes; twice that.
ceiling=30082066
base=$(sed -nE 's/^FROM (gcr\.io\/distroless\/[^ ]+).*/\1/p' deploy/Containerfile)
version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
work=$(mktemp -d)
name=fitment-smoke-$$
cleanup() {
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker volume rm "$name-catalog" "$name-state" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
fail() { echo "VT29: $*" >&2; exit 1; }

files() { # image, out: the image's file list
  local id
  id=$(docker create --entrypoint /none "$1")
  docker export "$id" | tar -t | sort >"$2"
  docker rm "$id" >/dev/null
}
files "$image" "$work/image.txt"
docker pull --quiet "$base" >/dev/null
files "$base" "$work/base.txt"
# The base ships gcc's GDB pretty-printers (usr/share/gcc/python/*.py), no
# interpreter: the whole image is scanned for an interpreter, uv or Open
# CASCADE, and what the image adds to its base for any of their names.
if grep -iE '(^|/)bin/(python[0-9.]*|uvx?)$|libTK|OCCT|opencascade' "$work/image.txt"; then
  fail "the image holds Python, uv or Open CASCADE"
fi
comm -23 "$work/image.txt" "$work/base.txt" >"$work/added.txt"
if grep -iE 'python|(^|/)uvx?(/|$)|libTK|OCCT|opencascade' "$work/added.txt"; then
  fail "the image adds Python, uv or Open CASCADE"
fi
grep -E '\.so(\.[0-9]+)*$' "$work/image.txt" >"$work/image-so.txt" || true
grep -E '\.so(\.[0-9]+)*$' "$work/base.txt" >"$work/base-so.txt" || true
extra=$(comm -23 "$work/image-so.txt" "$work/base-so.txt")
[ -z "$extra" ] || fail "shared libraries beyond the base's: $extra"
for file in usr/local/bin/fitment-server usr/local/bin/fitment usr/share/doc/fitment/LICENSE \
  usr/share/doc/fitment/THIRD-PARTY.txt etc/fitment/server.toml; do
  grep -qx "$file" "$work/image.txt" || fail "the image lacks $file"
done

user=$(docker image inspect --format '{{.Config.User}}' "$image")
[ "$user" = 65532 ] || fail "the image's user is '$user', not 65532"
[ "$(docker run --rm --entrypoint fitment-server "$image" --version)" = "fitment-server $version" ] ||
  fail "fitment-server --version"
[ "$(docker run --rm --entrypoint fitment "$image" --version)" = "fitment $version" ] ||
  fail "fitment --version"

docker volume create "$name-catalog" >/dev/null
docker volume create "$name-state" >/dev/null
volumes=(-v "$name-catalog:/var/lib/fitment/catalog" -v "$name-state:/var/lib/fitment/state")
docker run --rm --read-only "${volumes[@]}" --entrypoint fitment "$image" \
  catalog init -C /var/lib/fitment/catalog
docker run -d --name "$name" --read-only "${volumes[@]}" -p 127.0.0.1::8080 "$image" >/dev/null
port=$(docker port "$name" 8080/tcp | head -1 | sed 's/.*://')
for _ in $(seq 1 60); do
  status=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/v1/health" || true)
  [ "$status" = 204 ] && break
  sleep 0.5
done
[ "$status" = 204 ] || { docker logs "$name" >&2; fail "health answered '$status'"; }
[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/v1/ready")" = 204 ] ||
  fail "not ready"
curl -sf "http://127.0.0.1:$port/v1/info" | grep -q "\"version\": \"$version\"" ||
  fail "fitment.info does not name $version"
docker exec "$name" fitment-server healthcheck || fail "the image's healthcheck failed"
docker stop --time 45 "$name" >/dev/null
code=$(docker inspect --format '{{.State.ExitCode}}' "$name")
[ "$code" = 0 ] || { docker logs "$name" >&2; fail "the server exited $code after SIGTERM"; }
docker logs "$name" 2>/dev/null | grep -qx "fitment-server: listening on 0.0.0.0:8080" ||
  fail "stdout is not the address line"

size=$(docker image inspect --format '{{.Size}}' "$image")
echo "VT29: the image is $size bytes (ceiling $ceiling)"
[ "$size" -le "$ceiling" ] || fail "the image is over its ceiling"
echo "VT29: ok"
