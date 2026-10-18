#!/usr/bin/env bash
# Every catalog command of fitment 0.2.0, with each of its formats, run by
# two fitment binaries over the committed parts and the corpus, each in a
# directory of its own under the same relative names; their stdout,
# stderr, exit codes and files written compared byte for byte
# (docs/research/m6-api.md, VT39 and PR 7's "done when": the CLI's
# documents of 0.2.0 commands are unchanged).
#
#   tools/cli-compare.sh OLD_FITMENT NEW_FITMENT [CORPUS_DIR] [OUT_DIR]
#
# CORPUS_DIR defaults to corpus/out (tools/gen-corpus.py writes it); OUT_DIR
# keeps both runs' outputs (default: a temporary directory, removed). It
# prints the runs compared and each run that differs, and exits 1 if any
# does. Build the old binary from its release commit, for example:
#
#   git archive 7207750 | tar -x -C /tmp/v020
#   (cd /tmp/v020 && cargo build --release -p fitment --locked)
set -euo pipefail
cd "$(dirname "$0")/.."
old=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
new=$(cd "$(dirname "$2")" && pwd)/$(basename "$2")
corpus=${3:-corpus/out}
keep=${4:-}
if [ -n "$keep" ]; then
  mkdir -p "$keep"
  root=$(cd "$keep" && pwd)
else
  root=$(mktemp -d)
  trap 'chmod -R u+w "$root" 2>/dev/null; rm -rf "$root"' EXIT
fi
parts=$(pwd)/crates/fitment-core/tests
corpus=$(cd "$corpus" && pwd)

# One binary's runs, in $root/$name: inputs under in/, each run's stdout,
# stderr and exit code under out/.
runs() {
  local name=$1 fitment=$2
  local work=$root/$name
  chmod -R u+w "$work" 2>/dev/null || true
  rm -rf "$work"
  mkdir -p "$work/in" "$work/out" "$work/files"
  cd "$work"
  cp -R "$parts/data" "$parts/candidates" "$parts/assemblies" in/
  mkdir in/corpus in/extra
  cp "$corpus"/*.stp in/corpus/
  # Brackets no other file holds: each header names another file.
  local variant
  for variant in next brkt brkt2; do
    perl -pe "\$d ||= s/FILE_NAME\\(\\x27/FILE_NAME(\\x27$variant/" in/candidates/m_bracket.stp \
      >"in/extra/m_bracket_$variant.stp"
  done
  printf 'not STEP' >in/extra/junk.stp
  local n=0
  run() {
    n=$((n + 1))
    local key
    key=$(printf '%04d' "$n")
    printf '%s\n' "$*" >"out/$key.args"
    local code=0
    "$fitment" "$@" >"out/$key.out" 2>"out/$key.err" || code=$?
    echo "$code" >"out/$key.code"
  }
  run catalog init -C cat
  run catalog init -C cat
  run catalog init -C in/data
  run catalog ingest -C cat in/data --format json
  run catalog ingest -C cat in/candidates
  run catalog ingest -C cat in/assemblies --format json
  run catalog ingest -C cat in/corpus --format json
  run catalog ingest -C cat in/corpus
  run catalog ingest -C cat --id BRKT in/candidates/m_bracket.stp
  run catalog ingest -C cat --id BRKT in/extra/m_bracket_brkt.stp --format json
  run catalog ingest -C cat --id BRKT in/data/p01_through.stp in/data/p03_blind_flat.stp
  run catalog ingest -C cat --id 'bad id' in/data/p01_through.stp
  run catalog ingest -C cat cat/catalog.db
  run catalog ingest -C cat in/extra/junk.stp --format json
  run catalog ingest -C cat nowhere.stp
  run catalog ingest -C cat nowhere.stp --format json
  run catalog ingest -C cat --max-mib 0 in/data/p01_through.stp
  for status in all indexed quarantined stale; do
    run catalog list -C cat --status "$status"
    run catalog list -C cat --status "$status" --format json
  done
  local ids
  ids=$("$fitment" catalog list -C cat 2>/dev/null | awk 'NR > 1 { print $1 }')
  local id
  for id in $ids; do
    run catalog get -C cat "$id"
    run catalog get -C cat "$id" --format json
    run catalog get -C cat "$id" --format features
  done
  for id in $(echo "$ids" | head -40); do
    run catalog get -C cat "$id" --format step -o "files/$id.stp"
  done
  run catalog get -C cat m_bracket --format step
  run catalog get -C cat m_bracket --format step -o cat/x.stp
  run catalog get -C cat nosuch
  run catalog get -C cat m_bracket@r9
  run catalog get -C cat 'm_bracket@R1'
  run catalog get -C cat sha256:1b77ea9724e6
  run catalog get -C cat sha256:1b77
  for target in "s01_bolted BRACKET" "s01_bolted NUT" "s02_cbore BLOCK" "s03_dowels PLATE"; do
    set -- $target
    run socket "in/assemblies/$1.stp" --part "$2" -o "files/$1-$2.json"
    run match "files/$1-$2.json" -C cat
    run match "files/$1-$2.json" -C cat --format json
    run match "files/$1-$2.json" -C cat --exhaustive --format json
    run match "files/$1-$2.json" -C cat --id m_bracket --id BRKT --format json
    run match "files/$1-$2.json" -C cat --clearance functional --jobs 4 --format json
    run match "files/$1-$2.json" -C cat --rejects all --explain
  done
  run catalog replace -C cat m_bracket in/extra/m_bracket_next.stp
  run catalog replace -C cat m_bracket in/extra/m_bracket_next.stp --format json
  run catalog replace -C cat m_bracket@r1 in/candidates/m_bracket.stp
  run catalog replace -C cat m_bracket in/extra/junk.stp
  run catalog replace -C cat m_bracket in/data/p01_through.stp
  run catalog replace -C cat BRKT in/candidates/m_bracket.stp --format json
  run catalog replace -C cat BRKT@r1 in/extra/m_bracket_brkt2.stp --format json
  run catalog get -C cat m_bracket@r1
  run catalog get -C cat m_bracket@r1 --format json
  run catalog get -C cat m_bracket@r1 --format step -o files/r1.stp
  run catalog reindex -C cat
  run catalog reindex -C cat --format json
  run catalog reindex -C cat --all --format json
  run catalog reindex -C cat m_bracket p01_through
  run catalog reindex -C cat nosuch
  run catalog prune -C cat
  run catalog prune -C cat --revisions
  run catalog prune -C cat --revisions
  run catalog get -C cat m_bracket@r1 --format step -o files/r1-pruned.stp
  run catalog delete -C cat nosuch p01_through
  run catalog delete -C cat sha256:1b77ea9724e6
  run catalog delete -C cat p01_through BRKT@r2 p03_blind_flat
  run catalog delete -C cat BRKT
  run catalog get -C cat p01_through --format json
  run catalog verify -C cat
  run catalog verify -C cat --full
  run catalog verify -C cat --deep
  run catalog gc -C cat
  run catalog gc -C cat --versions
  run catalog gc -C cat --vacuum
  # A blob altered behind the catalog's back: verify finds it, get refuses
  # to hand it out (ST11).
  local blob
  blob=$(find cat/blobs/sha256 -type f | LC_ALL=C sort | head -1)
  chmod u+w "$blob"
  printf 'other' >"$blob"
  run catalog verify -C cat
  run catalog verify -C cat --deep
  for id in $(echo "$ids" | head -40); do
    run catalog get -C cat "$id" --format step -o "files/again-$id.stp"
  done
  run catalog list -C cat --format json
  cd - >/dev/null
}

runs old "$old"
runs new "$new"
total=$(find "$root/old/out" -name '*.code' | wc -l | tr -d ' ')
differ=0
for code in "$root"/old/out/*.code; do
  key=$(basename "$code" .code)
  same=1
  for part in code out err; do
    cmp -s "$root/old/out/$key.$part" "$root/new/out/$key.$part" || same=0
  done
  if [ "$same" = 0 ]; then
    differ=$((differ + 1))
    echo "differs: $(cat "$root/old/out/$key.args")"
    diff "$root/old/out/$key.out" "$root/new/out/$key.out" | sed 's/^/  stdout /' | head -6 || true
    diff "$root/old/out/$key.err" "$root/new/out/$key.err" | sed 's/^/  stderr /' | head -6 || true
  fi
done
files=$(find "$root/old/files" -type f | wc -l | tr -d ' ')
if ! diff -r "$root/old/files" "$root/new/files" >/dev/null; then
  echo "differs: the files written"
  differ=$((differ + 1))
fi
echo "$total runs and $files files written compared: $differ differ"
[ "$differ" = 0 ]
