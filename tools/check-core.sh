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

# No async code, so no runtime can creep in.
check "contains async code" \
  grep -rnE '\basync\b|\.await\b' "$src"

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
cargo package --workspace --exclude fitment-corpus --no-verify --locked --allow-dirty --quiet
for package in fitment-core fitment fitment-catalog; do
  tree=$(cargo tree -p "$package" -e normal,build --all-features --locked --prefix none)
  check "fitment-corpus is in $package's normal graph" \
    grep -q '^fitment-corpus ' <<<"$tree"
  manifest=$(tar -xzOf "$target/package/$package-$version.crate" "$package-$version/Cargo.toml")
  check "the packaged $package names fitment-corpus" \
    grep -q 'fitment-corpus' <<<"$manifest"
done

cargo deny --manifest-path crates/fitment-core/Cargo.toml --config deny/core.toml check bans
python3 tools/core-deps.py

exit "$failed"
