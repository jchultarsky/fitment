#!/usr/bin/env bash
# Download public STEP test files into fixtures/. Copied from stepq's
# tools/fetch-fixtures.sh, with the same URLs and SHA-256 pins; only the
# destination differs.
#
# Usage: tools/fetch-fixtures.sh [SET...]
#   core   STEP Tools AS1 and NIST (~20 MB). What CI's fixture test uses.
#   ocp    Open Compute Project reference assemblies (~130 MB download,
#          ~860 MB unpacked; up to 3.5 million instances per file).
#   all    Both (the default).
#
# Fixtures are not vendored: the NIST sets are US-Government public domain,
# but the other sources carry no redistribution terms. See
# fixtures/README.md for sources and terms, and fixtures/manifest.json for
# what fitment's gate makes of each core file.
#
# Every download is checked against a pinned SHA-256; a mismatch means the
# upstream file changed and fails the script rather than testing against
# something new. Update the pin deliberately after looking at the change.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dest="$here/fixtures"

sha256() {
  if command -v sha256sum >/dev/null; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

fetch() {
  local url="$1" out="$2" pin="$3"
  mkdir -p "$(dirname "$out")"
  if [[ -s "$out" && "$(sha256 "$out")" == "$pin" ]]; then
    echo "have    ${out#"$dest"/}"
    return
  fi
  echo "fetch   ${out#"$dest"/}"
  curl -fsSL --retry 3 "$url" -o "$out.part"
  local got
  got="$(sha256 "$out.part")"
  if [[ "$got" != "$pin" ]]; then
    rm -f "$out.part"
    echo "error: ${out#"$dest"/}: SHA-256 $got, expected $pin" >&2
    echo "       from $url" >&2
    exit 1
  fi
  mv "$out.part" "$out"
}

# Unpacks an archive into a directory named after it, once.
unpack() {
  local archive="$1" dir="${1%.*}"
  [[ -d "$dir" ]] && return
  echo "unpack  ${archive#"$dest"/}"
  mkdir -p "$dir.part"
  case "$archive" in
    *.zip) unzip -q -o "$archive" -d "$dir.part" ;;
    *.7z)
      if command -v 7z >/dev/null; then
        7z x -y -bd -o"$dir.part" "$archive" >/dev/null
      else
        bsdtar -xf "$archive" -C "$dir.part"
      fi
      ;;
  esac
  mv "$dir.part" "$dir"
}

core() {
  # STEP Tools: the AS1 assembly as written by five different exporters.
  # Same geometry, five dialects — ideal for differential testing.
  local st="https://steptools.com/docs/stpfiles/ap214"
  fetch "$st/as1-ac-214.stp" "$dest/steptools/as1-ac-214.stp" 1bb1a0e55dc4a0169e329529eb520596690f8745077c1bdd2f9fa41fa5a2aa95
  fetch "$st/as1-ec-214.stp" "$dest/steptools/as1-ec-214.stp" c835eac6fa9ec51b612ed570da59d1872f8bc537195d86ed5cf039914b855b1d
  fetch "$st/as1-md-214.stp" "$dest/steptools/as1-md-214.stp" 208e8eb1fd95f564f5a5c0ed60f6539537edda6303ca7c60854bb292e2873bbc
  fetch "$st/as1-tc-214.stp" "$dest/steptools/as1-tc-214.stp" 4978cd2cf99bd5a5e9805bcdfd05c17f148f4a4a107b105f5446dff2ba753310
  fetch "$st/as1-ug-214.stp" "$dest/steptools/as1-ug-214.stp" 53c5069f5d6b59435eb32ac743cc82e53bd3607954c94785d989d579d0e29426

  # NIST MBE PMI validation models (CTC/FTC test cases): single parts in
  # AP203 and AP242, with semantic and graphical PMI. ~14 MB.
  # The /document/ link redirects to the current upload.
  fetch "https://www.nist.gov/document/nist-pmi-step-files" "$dest/nist/NIST-PMI-STEP-Files.zip" \
    8fa78429e6d8d9b0d7681d223b6aa9ec98c3772185c55b1a0e3679b21c181911
  if [[ ! -d "$dest/nist/NIST-PMI-STEP-Files" ]]; then
    echo "unpack  nist/NIST-PMI-STEP-Files.zip"
    unzip -q -o "$dest/nist/NIST-PMI-STEP-Files.zip" -d "$dest/nist"
  fi

  # NIST Engineering Design Model Repository: 1990s AP203 files, published
  # by NIST as public domain. as1_pe is the same AS1 model as the STEP Tools
  # set, written by Pro/ENGINEER, so it doubles as an open nested assembly.
  local edm="https://raw.githubusercontent.com/usnistgov/engineering-design-models/master/models/STEP/STI"
  fetch "$edm/as1_pe/as1_pe.stp" "$dest/nist-edm/as1_pe.stp" 5a46af594d0e3a192aad66fc27d21a8edae32c96c1a9b6185a143787e4874b65
  fetch "$edm/moon_buggy_asm/moon_buggy_asm.stp" "$dest/nist-edm/moon_buggy_asm.stp" 5879852f690bd486c32a2ddf4f4d11e355bbf3550528530e73eaf01a17669cbf
  fetch "$edm/vaccase_asm_solid/vaccase_asm_solid.stp" "$dest/nist-edm/vaccase_asm_solid.stp" a2127a07f6bb2e03b02f1a42134360549dfdaf08381fe08095781b15e103c5dc
  fetch "$edm/weldment_asm_solid/weldment_asm_solid.stp" "$dest/nist-edm/weldment_asm_solid.stp" a99e0cac838b2b6699094921965c9b430b05e8d073ed0438de325cb16fb5b8d4
  fetch "$edm/clevis21/clevis21.stp" "$dest/nist-edm/clevis21.stp" 4fff420338c430410ff289fef8e9b90c182aea18b7ae38817670262a846ee55e
  fetch "$edm/valve/valve.stp" "$dest/nist-edm/valve.stp" b97a2f11a27bf1672dc63299fb48b5f74b4fe9fe934c87578e3fcfca9d93eeb7
  fetch "$edm/gear/gear.stp" "$dest/nist-edm/gear.stp" 4c7ecc5471cd79798f7253810fdc3c261d4ee1624ca721e709b2040b2c369a17
}

ocp() {
  # Open Rack V3 reference CAD (Creo, AP214), linked from the Open Compute
  # Project wiki's Open Rack "Reference CAD" section. Google Drive shares:
  # the checksum also catches Drive answering with an HTML page instead.
  local drive="https://drive.usercontent.google.com/download?export=download&id="
  fetch "${drive}1RsWDCzrWCKpbWGG8FvDLlcvEGAnjSVId" "$dest/ocp/OCP_v3_Enclosure_6OU_Section_c30001_ARP_2022.stp" \
    f74a4ee38fd30cbd1e114aa551a64c7c7079024d39eab52158bc78469be8dcf0
  fetch "${drive}1v29mj4MlykyFwuU8bjU54QWVJsUwUB3S" "$dest/ocp/ORv3_PSU_Mechanical.zip" \
    e226b4caccd656dbb450c753b0846f1bd98a418f6b835fe6c8e7629e96714229
  fetch "${drive}1EZOFyB9utlPYRp8x4_M5MG9rhVu2D_k4" "$dest/ocp/ORv3_BBU_Mechanical.zip" \
    60ad312184a88dfaab5cdefb57171224bba89418fc8605934790133ab18fc6a9

  # Project Olympus (Microsoft) mechanical assemblies (Creo, AP203),
  # ~200 MB of STEP each once unpacked.
  local olympus="https://raw.githubusercontent.com/opencomputeproject/Project_Olympus/master/HW"
  fetch "$olympus/ProjectOlympusChassis20170410.7z" "$dest/ocp/ProjectOlympusChassis20170410.7z" \
    050760cc34794476a32a446d0cee453b08e0078bb3e0b58d43c6d97f8c5730f9
  fetch "$olympus/ProjectOlympusComputeServer20170410.7z" "$dest/ocp/ProjectOlympusComputeServer20170410.7z" \
    ded8851a6466696913d19c2e82c2c214ac87c835c340dc81408dfa4122a049a3
  fetch "$olympus/ProjectOlympusUniversalMotherboard20170410.7z" "$dest/ocp/ProjectOlympusUniversalMotherboard20170410.7z" \
    e63a379a9ff8ad1d292c2d14974b0ceb6935038c38b491242bce36d3a61e0309

  local archive
  for archive in "$dest"/ocp/*.zip "$dest"/ocp/*.7z; do
    unpack "$archive"
  done
}

sets=("$@")
[[ ${#sets[@]} -eq 0 ]] && sets=(all)
for set in "${sets[@]}"; do
  case "$set" in
    core) core ;;
    ocp) ocp ;;
    all) core; ocp ;;
    *) echo "unknown fixture set '$set' (core, ocp, all)" >&2; exit 2 ;;
  esac
done

echo
echo "fixtures ready in $dest"
find "$dest" -type f \( -iname '*.stp' -o -iname '*.step' \) | wc -l | xargs echo "STEP files:"
