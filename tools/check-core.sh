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

cargo deny --manifest-path crates/fitment-core/Cargo.toml --config deny/core.toml check bans
python3 tools/core-deps.py

exit "$failed"
