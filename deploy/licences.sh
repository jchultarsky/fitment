#!/bin/sh
# The image's third-party notices (docs/research/m6-api.md, OP8; PR 2's
# review S5), run in the build stage of deploy/Containerfile with nothing
# but cargo and the base system's sh, find, sort and cp:
#
#   deploy/licences.sh OUT
#
# For every crate of the two binaries' locked normal graph, OUT/licenses/
# <name>-<version>/ receives the package's own LICENSE*, LICENCE*, COPYING*,
# NOTICE* and UNLICENSE* files, as published; OUT/THIRD-PARTY.txt lists
# each crate once with its declared licence and the files shipped, and
# names no build path. The workspace's own crates are this repository's,
# under its LICENSE.
set -eu
out=$1
mkdir -p "$out/licenses"
registry="${CARGO_HOME:-$HOME/.cargo}/registry/src"
list=$(mktemp)
cargo tree --locked --edges normal --prefix none --format '{p}|{l}' \
  -p fitment-server -p fitment | sed -E 's/ \([^)]*\)//g' | sort -u >"$list"
{
  echo "Third-party crates in fitment-server and fitment, with their licences."
  echo "Each crate's own licence and notice files are in licenses/<crate>-<version>/."
  echo
} >"$out/THIRD-PARTY.txt"
missing=0
while IFS='|' read -r package licence; do
  [ -n "$package" ] || continue
  name=${package% v*}
  version=${package##* v}
  case $name in
    fitment | fitment-core | fitment-catalog | fitment-fit | fitment-server)
      echo "$name $version: this repository, $licence (LICENSE)" >>"$out/THIRD-PARTY.txt"
      continue ;;
  esac
  dir=$(find "$registry" -mindepth 2 -maxdepth 2 -type d -name "$name-$version" | head -n 1)
  files=""
  if [ -n "$dir" ]; then
    for file in "$dir"/LICENSE* "$dir"/LICENCE* "$dir"/COPYING* "$dir"/NOTICE* "$dir"/UNLICENSE* \
                "$dir"/license* "$dir"/licence*; do
      [ -f "$file" ] || continue
      # Once each (a case-insensitive file system matches twice).
      case "$files " in *" ${file##*/} "*) continue ;; esac
      mkdir -p "$out/licenses/$name-$version"
      cp "$file" "$out/licenses/$name-$version/"
      files="$files ${file##*/}"
    done
  fi
  if [ -z "$files" ]; then
    files=" (the package ships no licence file; its manifest declares the licence)"
    missing=$((missing + 1))
  fi
  echo "$name $version: $licence:$files" >>"$out/THIRD-PARTY.txt"
done <"$list"
rm -f "$list"
echo "THIRD-PARTY.txt: $(grep -c ': ' "$out/THIRD-PARTY.txt") crates, $missing without a licence file" >&2
