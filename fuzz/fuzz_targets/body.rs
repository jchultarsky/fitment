//! VT26's body target (docs/research/m6-api.md, M6-42): arbitrary bytes
//! as the body of every request type, decoded and validated; an accepted
//! body written again is accepted as the same value with the same JB9
//! fingerprint (`fitment_server::fuzzing::body`).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    fitment_server::fuzzing::body(input);
});
