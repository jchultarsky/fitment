use sha2::Digest;
fn main() {
    let b = std::fs::read("/Users/julian/projects/fitment/corpus/out/s55_tiles.stp").unwrap();
    let t = std::time::Instant::now(); let mut x = String::new(); for _ in 0..10 { x = fitment_core::sha256_hex(&b); }
    let core = t.elapsed().as_secs_f64() / 10.0;
    let t = std::time::Instant::now(); let mut y = String::new(); for _ in 0..10 { y = sha2::Sha256::digest(&b).iter().map(|v| format!("{v:02x}")).collect(); }
    let s2 = t.elapsed().as_secs_f64() / 10.0;
    assert_eq!(x, y);
    println!("{} bytes: fitment_core::sha256 {:.0} MB/s, sha2 {:.0} MB/s", b.len(), b.len() as f64 / core / 1e6, b.len() as f64 / s2 / 1e6);
}
