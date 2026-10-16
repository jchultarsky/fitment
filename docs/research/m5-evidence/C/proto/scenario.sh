#!/bin/sh
# Scenario for the prototype: real gate, real fixtures. Usage: sh scenario.sh REPO WORK
REPO=$1; W=$2
F=$REPO/target/release/fitment
P="python3 -I $(dirname $0)/proto_catalog.py $F"
rm -rf "$W"; mkdir -p "$W/in"
run() { echo "\$ catalog $*"; $P -C "$W/cat" "$@"; echo "[exit $?]"; }
run list
run init
run init
mkdir -p "$W/other" && touch "$W/other/x"; $P -C "$W/other" init; echo "[exit $? init on a non-empty dir]"
C=$REPO/crates/fitment-core/tests
cp $C/candidates/m_bracket.stp $C/candidates/m_bracket_missing_hole.stp $C/data/r04_assembly.stp "$W/in/"
cp $C/candidates/m_bracket.stp "$W/in/copy of m_bracket.stp"
cp $C/data/p01_through.stp "$W/in/a b.stp"; cp $C/data/p03_blind_flat.stp "$W/in/a_b.stp"
printf 'ISO-10303-21;\nnot step\n' > "$W/in/junk.stp"
cp $C/data/p05_cbore.stp "$W/in/$(printf 'evil\033[31mred\342\200\256.stp')"
run add "$W/in"
run add "$W/in"
run add --id BRKT-1 $C/candidates/m_bracket.stp
N=$REPO/fixtures/nist/NIST-PMI-STEP-Files
run add $N/nist_ctc_05_asme1_ap242-e1.stp $N/nist_stc_09_asme1_ap242-e3.stp $N/nist_ctc_04_asme1_ap242-e1.stp
run add $REPO/fixtures/nist-edm/clevis21.stp $REPO/fixtures/nist-edm/gear.stp
run list
run quarantine
run get nist_stc_09_asme1_ap242-e3
run replace m_bracket $C/candidates/m_bracket_thick.stp
printf "ISO-10303-21;\nother junk\n" > "$W/junk2.stp"
run replace m_bracket $W/junk2.stp
run get m_bracket
run remove m_bracket nosuch
run remove m_bracket
run add $C/candidates/m_bracket.stp
run get M_BRACKET
run verify
touch "$W/cat/blobs/tmp/leftover.1234"
B=$(ls $W/cat/blobs/4f/4fff*); chmod 644 $B; echo garbage >> $B
run verify
PROTO_EXTRACTOR=7 $P -C "$W/cat" list; echo "[exit $? list under a newer extractor]"
