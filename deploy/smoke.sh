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
#    host and through the image's own `healthcheck`), the first admin is
#    made with `admin bootstrap` beside the running server (AU9) and its
#    token, printed once, reads `fitment.info`, which names the version and
#    needs a token (AU5); a second bootstrap is refused; s01 and the
#    committed bracket are uploaded under their SHA-256s (UP2) and found
#    by HEAD and GET as the caller's (UP3), a wrong name keeps nothing and
#    POST names the bytes itself; s01 is registered and its BRACKET's
#    socket extracted through jobs (PR 5), each waited for, their documents
#    the CLI's goldens' bytes (crates/fitment/tests/golden/{parts,socket}/);
#    the token is in no log; and SIGTERM stops it with exit 0. PR 6 adds a
#    match job whose document equals crates/fitment/tests/golden/match/'s.
# 4. Its size is reported, under a ceiling of twice the first measured.
set -euo pipefail
cd "$(dirname "$0")/.."
image=${1:-fitment-server:ci}
# The first measured: 15,041,033 bytes on arm64 and 37,699,577 on amd64
# (CI, 2026-10-17), the larger of the two architectures; twice that.
ceiling=75399154
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
# S5 of PR 2's review: each crate's licence texts, not only their names.
grep -qE '^usr/share/doc/fitment/licenses/tokio-[0-9.]+/LICENSE$' "$work/image.txt" ||
  fail "the image lacks the crates' licence texts"
id=$(docker create --entrypoint /none "$image")
notices=$(docker cp "$id:/usr/share/doc/fitment/THIRD-PARTY.txt" - | tar -xO)
docker rm "$id" >/dev/null
echo "$notices" | grep -q '^tokio ' || fail "THIRD-PARTY.txt does not list tokio"
if echo "$notices" | grep -qE '/src/|/usr/local/cargo|registry/src'; then
  fail "THIRD-PARTY.txt names a build path"
fi

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
# AU9: the first admin, offline, beside the running server; its token on
# stdout once.
token=$(docker exec "$name" fitment-server admin bootstrap --username smoke)
case "$token" in
  fitment_pat_*) ;;
  *) fail "admin bootstrap printed no token" ;;
esac
if docker exec "$name" fitment-server admin bootstrap --username again >/dev/null 2>&1; then
  fail "a second bootstrap was not refused"
fi
[ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/v1/info")" = 401 ] ||
  fail "fitment.info answered without a token"
auth=(-H "Authorization: Bearer $token")
curl -sf "${auth[@]}" "http://127.0.0.1:$port/v1/info" | grep -q "\"version\": \"$version\"" ||
  fail "fitment.info does not name $version"
curl -sf "${auth[@]}" "http://127.0.0.1:$port/v1/auth/me" | grep -q '"role": "admin"' ||
  fail "the bootstrap token is not an admin's"
# UP2, UP3: uploads under their SHA-256s, found as the caller's own.
sha() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}
uploads="http://127.0.0.1:$port/v1/uploads"
for file in crates/fitment-core/tests/assemblies/s01_bolted.stp crates/fitment-core/tests/candidates/m_bracket.stp; do
  hash=$(sha "$file")
  status=$(curl -s -o "$work/upload.json" -w '%{http_code}' -X PUT "${auth[@]}" \
    -H 'Content-Type: model/step' --data-binary "@$file" "$uploads/sha256:$hash")
  [ "$status" = 201 ] || { cat "$work/upload.json" >&2; fail "uploading $file answered $status"; }
  grep -q "\"sha256\": \"$hash\"" "$work/upload.json" || fail "the upload of $file is not named $hash"
  [ "$(curl -s -o /dev/null -w '%{http_code}' -I "${auth[@]}" "$uploads/sha256:$hash")" = 200 ] ||
    fail "HEAD does not find $file"
  curl -sf "${auth[@]}" "$uploads/sha256:$hash" | grep -q '"held_by": \[' || fail "GET of $file"
done
wrong=$(sha crates/fitment-core/tests/assemblies/s02_cbore.stp)
status=$(curl -s -o /dev/null -w '%{http_code}' -X PUT "${auth[@]}" -H 'Content-Type: model/step' \
  --data-binary @crates/fitment-core/tests/assemblies/s01_bolted.stp "$uploads/sha256:$wrong")
[ "$status" = 422 ] || fail "a wrong name answered $status"
[ "$(curl -s -o /dev/null -w '%{http_code}' -I "${auth[@]}" "$uploads/sha256:$wrong")" = 404 ] ||
  fail "a wrong name kept something"
location=$(curl -s -D - -o /dev/null -X POST "${auth[@]}" -H 'Content-Type: model/step' \
  --data-binary @crates/fitment-core/tests/candidates/m_bracket.stp "$uploads" |
  tr -d '\r' | sed -nE 's/^[Ll]ocation: //p')
[ "$location" = "/v1/uploads/sha256:$(sha crates/fitment-core/tests/candidates/m_bracket.stp)" ] ||
  fail "POST named the bracket '$location'"
# PR 5: an assembly_read and a socket_extract job, waited for (JB11),
# their documents the CLI's bytes (DC1).
api="http://127.0.0.1:$port/v1"
s01=$(sha crates/fitment-core/tests/assemblies/s01_bolted.stp)
job() { # path, body, out: a job made with Prefer: wait, its id
  curl -sf -X POST "${auth[@]}" -H 'Content-Type: application/json' -H 'Prefer: wait=60' \
    --data "$2" "$api/$1" >"$3" || fail "POST /v1/$1"
  grep -q '"state": "succeeded"' "$3" || { cat "$3" >&2; fail "the job of /v1/$1 did not succeed"; }
  sed -nE 's/^  "id": "(j_[0-9a-z]+)",$/\1/p' "$3"
}
id=$(job assemblies "{\"upload\": \"sha256:$s01\", \"file_name\": \"s01_bolted.stp\"}" "$work/assembly.json")
curl -sf "${auth[@]}" "$api/jobs/$id/document" -o "$work/parts.json" || fail "the parts document"
cmp -s "$work/parts.json" crates/fitment/tests/golden/parts/s01_bolted.json ||
  fail "the assembly's fitment.parts is not the CLI's"
id=$(job sockets "{\"assembly\": \"sha256:$s01\", \"part\": \"BRACKET\"}" "$work/socket.json")
curl -sf "${auth[@]}" "$api/jobs/$id/document" -o "$work/socket-doc.json" || fail "the socket document"
cmp -s "$work/socket-doc.json" "crates/fitment/tests/golden/socket/s01_bolted--BRACKET.json" ||
  fail "the socket is not the CLI's"
href=$(sed -nE 's/^    "href": "(\/v1\/sockets\/sk_[0-9a-f]+@r1)",$/\1/p' "$work/socket.json")
[ -n "$href" ] || fail "the socket job names no revision"
curl -sf "${auth[@]}" "http://127.0.0.1:$port$href" | cmp -s - "$work/socket-doc.json" ||
  fail "the revision is not the job's document"
docker exec "$name" fitment-server healthcheck || fail "the image's healthcheck failed"
docker stop --time 45 "$name" >/dev/null
code=$(docker inspect --format '{{.State.ExitCode}}' "$name")
[ "$code" = 0 ] || { docker logs "$name" >&2; fail "the server exited $code after SIGTERM"; }
docker logs "$name" 2>/dev/null | grep -qx "fitment-server: listening on 0.0.0.0:8080" ||
  fail "stdout is not the address line"
if docker logs "$name" 2>&1 | grep -qF "${token#fitment_pat_}"; then
  fail "the bootstrap token is in the server's log"
fi

size=$(docker image inspect --format '{{.Size}}' "$image")
echo "VT29: the image is $size bytes (ceiling $ceiling)"
[ "$size" -le "$ceiling" ] || fail "the image is over its ceiling"
echo "VT29: ok"
