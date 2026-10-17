#!/usr/bin/env bash
# The catalog's outputs for one fixed ingest sequence, for CI to compare
# across Linux, macOS and Windows (docs/research/m5-catalog.md, CT7): the
# `list` document, a full and an exhaustive match document, and `get`'s
# document. Each must be the same bytes on every OS.
#
#   tools/catalog-outputs.sh OUT_DIR [FITMENT]
set -euo pipefail
cd "$(dirname "$0")/.."
out=$1
fitment=${2:-target/debug/fitment}
rm -rf "$out"
mkdir -p "$out"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

parts=crates/fitment-core/tests
"$fitment" socket "$parts/assemblies/s01_bolted.stp" --part BRACKET -o "$work/bracket.json"
"$fitment" catalog init -C "$work/cat" 2>/dev/null
# The same files, in the same order, every time: a directory, then two
# files, one of them refused.
"$fitment" catalog ingest -C "$work/cat" "$parts/candidates" >/dev/null 2>&1
"$fitment" catalog ingest -C "$work/cat" "$parts/data/p01_through.stp" "$parts/data/r04_assembly.stp" >/dev/null 2>&1 || true
"$fitment" catalog list -C "$work/cat" --format json >"$out/list.json"
"$fitment" catalog get -C "$work/cat" m_bracket --format json >"$out/get.json"
code=0
"$fitment" match "$work/bracket.json" -C "$work/cat" --format json >"$out/full.json" || code=$?
echo "$code" >"$out/full.code"
code=0
"$fitment" match "$work/bracket.json" -C "$work/cat" --exhaustive --format json >"$out/exhaustive.json" || code=$?
echo "$code" >"$out/exhaustive.code"
