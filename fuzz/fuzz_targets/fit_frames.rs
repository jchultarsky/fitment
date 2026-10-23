//! VT75's frames target (docs/research/m7-fit.md, FH1, FH5): arbitrary
//! bytes through fitment-fit's frame reader, as either side reads frames,
//! and through every reply decoder, against requests whose nonce and
//! digests are the bytes' own and against fixed ones: no panic, no
//! allocation past FH1's caps (a buffer grows only with the bytes that
//! arrive), every rejection a protocol error
//! (`fitment_fit::testing::fuzz_frames`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    fitment_fit::testing::fuzz_frames(input);
});
