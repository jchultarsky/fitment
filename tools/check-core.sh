#!/usr/bin/env bash
# Checks that keep fitment-core free of I/O, global state, async code and
# unlisted dependencies, beyond what clippy.toml can express. CI runs this;
# run it before opening a PR. See CONTRIBUTING.md, "Repository layout".
set -euo pipefail
cd "$(dirname "$0")/.."

src=crates/fitment-core/src
code=(crates/fitment-core/src crates/fitment-core/tests)
test -d "$src"
failed=0

check() { # description, then a grep that must find nothing
  local what=$1; shift
  if "$@"; then
    echo "fitment-core: $what" >&2
    failed=1
  fi
}

# clippy.toml bans single items; these whole std modules are off limits.
check "uses a std module it must not (fs, net, env, process, thread, os)" \
  grep -rnE '\bstd::(fs|net|env|process|thread|os)\b|use std::\{[^}]*\b(fs|net|env|process|thread|os)\b' "${code[@]}"

# No async code, so no runtime can creep in. SV1 (docs/research/m6-api.md):
# fitment-server is the only async crate; the catalog and the CLI are
# synchronous too.
check "contains async code" \
  grep -rnE '\basync\b|\.await\b' "$src"
check "fitment-catalog or fitment contains async code" \
  grep -rnE '\basync\b|\.await\b' crates/fitment-catalog/src crates/fitment/src

# With unsafe forbidden, global mutable state needs a static item.
check "declares a static item (global state)" \
  grep -rnE "(^|[^'[:alnum:]_])static[[:space:]]+(mut[[:space:]]+)?[A-Z_]" "$src"

# stepq stays a private dependency, named only in the gate and the adapters, so a stepq
# release is a change in one place and its types never reach the public API.
check "names stepq outside gate/, features/step.rs, assembly/step.rs and properties/step.rs" \
  sh -c "grep -rlE '(^|[^[:alnum:]_])stepq::|use[[:space:]]+stepq' '$src' | grep -vxE '$src/(gate/[a-z_]+|features/step|assembly/step|properties/step)\.rs' | grep ."

# The tests hold STEP files that must never ship, in either package. Each
# listing runs on its own first, so that a failing `cargo package` fails the
# script (set -e) rather than look like a clean list.
for package in fitment-core fitment fitment-catalog; do
  packaged=$(cargo package --list -p "$package" --locked --allow-dirty)
  check "would publish a STEP file in $package" \
    grep -iE '\.(stp|step|p21)$' <<<"$packaged"
  check "would publish no LICENSE in $package" \
    sh -c "! grep -qx LICENSE <<'EOF'
$packaged
EOF"
done

# SC3 (docs/research/m6-api.md): fitment-corpus, the corpus's scorer, is
# never published and never reaches a published crate's normal graph; it is
# a dev-dependency without a version, which `cargo publish` strips, so the
# package lists above are unchanged by it.
check "fitment-corpus does not say publish = false" \
  sh -c "! grep -qx 'publish = false' crates/fitment-corpus/Cargo.toml"
# Each published crate's manifest as `cargo package` writes it must not name
# the scorer: the versionless dev-dependency is stripped there.
target=$(cargo metadata --format-version 1 --no-deps --locked |
  python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')
version=$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)
# The workspace at once, as CI's package job does: fitment's manifest names
# fitment-catalog, which only the workspace's local overlay holds.
# fitment-server is never published either (M6-35).
cargo package --workspace --exclude fitment-corpus --exclude fitment-server --no-verify --locked --allow-dirty --quiet
for package in fitment-core fitment fitment-catalog; do
  tree=$(cargo tree -p "$package" -e normal,build --all-features --locked --prefix none)
  check "fitment-corpus is in $package's normal graph" \
    grep -q '^fitment-corpus ' <<<"$tree"
  manifest=$(tar -xzOf "$target/package/$package-$version.crate" "$package-$version/Cargo.toml")
  check "the packaged $package names fitment-corpus" \
    grep -q 'fitment-corpus' <<<"$manifest"
done

# M6 (docs/research/m6-api.md, section 4.13). M6-35: fitment-server is
# never published and not a dist artifact.
server=crates/fitment-server
check "fitment-server does not say publish = false" \
  sh -c "! grep -qx 'publish = false' $server/Cargo.toml"
check "fitment-server does not say dist = false" \
  sh -c "! grep -A1 -x '\[package.metadata.dist\]' $server/Cargo.toml | grep -qx 'dist = false'"
# M6-43, OP8: the image is pushed only by image.yml, run by hand from a
# release's tag: no trigger but workflow_dispatch; `contents: read` at the
# top level and for `check` and `build`, which build and test the image;
# `packages: write` for `push` alone, which checks out nothing. Each block
# is read as written, every line indented under its key, so a quoted key,
# `key :`, a column-0 comment or flow style cannot hide an entry.
image=.github/workflows/image.yml
entries() { # a top-level key: its entries, comments and spaces dropped
  awk -v key="$1" '
    $0 ~ "^" key ":" { inside = 1; next }
    inside && /^[^[:space:]#]/ { exit }
    inside && !/^[[:space:]]*(#|$)/ { print }' "$image" |
    { grep -E '^  [^ ]' || true; } | sed -E 's/[[:space:]]+#.*//' | tr -d ' ' | tr '\n' ' '
}
triggers=$(entries on)
check "image.yml's triggers are not workflow_dispatch alone: $triggers" \
  test "$triggers" != "workflow_dispatch: "
# Every grant as `<top or job>=<entries>;`, in the file's order.
grants=$(awk '
  /^[[:space:]]*(#|$)/ { next }
  { match($0, /^ */); indent = RLENGTH }
  inside && indent > at { line = $0; sub(/[[:space:]]+#.*/, "", line)
                          gsub(/[[:space:]]/, "", line); got = got line " "; next }
  inside { printf "%s=%s;", where, got; inside = 0 }
  indent == 0 { top = $0 }
  indent == 2 && top == "jobs:" { job = $0; sub(/:.*/, "", job); gsub(/[[:space:]]/, "", job) }
  /^permissions:$/ { inside = 1; at = 0; where = "top"; got = ""; next }
  /^    permissions:$/ && top == "jobs:" { inside = 1; at = 4; where = job; got = ""; next }
  END { if (inside) printf "%s=%s;", where, got }' "$image")
check "image.yml's grants are not the four it may hold: $grants" \
  test "$grants" != "top=contents:read ;check=contents:read ;build=contents:read ;push=packages:write ;"
check "image.yml names permissions other than as a top-level or a job's key" \
  sh -c "grep -nE 'permissions' '$image' | grep -vE '^[0-9]+:((    )?permissions:|[[:space:]]*#.*)\$'"
# SV4: catalog.rs is the only module naming fitment_catalog (with its
# submodules under catalog/, as SV6 reads state::db), matching.rs the only
# one building a Matcher or a MatchDocument (the OpenAPI generator names the
# document's type for its schema only).
check "names fitment_catalog outside fitment-server's catalog.rs and catalog/" \
  sh -c "grep -rlE '\bfitment_catalog\b' $server/src | grep -vx '$server/src/catalog.rs' | grep -v '^$server/src/catalog/' | grep ."
check "builds a Matcher or a MatchDocument outside fitment-server's matching.rs" \
  sh -c "grep -rlE '\b(Matcher|MatchDocument)::' $server/src | grep -vx '$server/src/matching.rs' | grep ."
# SV6: server.db is opened by the module state::db alone (state/db.rs and
# its submodules under state/db/).
check "names rusqlite outside fitment-server's state/db.rs and state/db/" \
  sh -c "grep -rlE '\brusqlite\b' $server/src | grep -vx '$server/src/state/db.rs' | grep -v '^$server/src/state/db/' | grep ."
# SV7, DC1: documents are served as the library's bytes, never re-serialized
# through a Value; clippy cannot scope a ban to a module.
check "uses serde_json::(Value|from_str|to_value) in matching.rs or objects.rs" \
  sh -c "grep -nE 'serde_json::(Value|from_str|to_value)' $server/src/matching.rs $server/src/state/objects.rs 2>/dev/null | grep ."

# SV1, VT35: no async runtime or HTTP stack in the CLI's or the catalog's
# graph, whatever their features...
for package in fitment fitment-catalog; do
  tree=$(cargo tree -p "$package" -e normal --all-features --locked --prefix none)
  check "$package's graph holds an async crate" \
    grep -qE '^(tokio|mio|hyper|axum|tower|async-std|smol|futures-executor) ' <<<"$tree"
  cargo deny --manifest-path "crates/$package/Cargo.toml" --config deny/sync.toml check bans
done
# ...and SV9: the server's default runtime graph holds no schemars, no h2,
# no TLS stack, and serde_json with its three features exactly.
tree=$(cargo tree -p fitment-server -e normal --locked --prefix none)
check "fitment-server's runtime graph holds schemars, h2, a TLS stack or reqwest" \
  grep -qE '^(schemars|h2|rustls|ring|aws-lc-rs|aws-lc-sys|native-tls|openssl|reqwest) ' <<<"$tree"
features=$(cargo tree -p fitment-server -e normal,features --locked --prefix none |
  sed -nE 's/^serde_json feature "([^"]+)".*/\1/p' | sort -u | tr '\n' ' ')
check "serde_json's features in fitment-server's runtime graph are not default, float_roundtrip, std: $features" \
  test "$features" != "default float_roundtrip std "

cargo deny --manifest-path crates/fitment-core/Cargo.toml --config deny/core.toml check bans
python3 tools/core-deps.py

exit "$failed"
