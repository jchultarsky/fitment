use std::io::Write;
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    for threads in [1usize, 4, 8] {
        let _ = std::fs::remove_dir_all(&root); std::fs::create_dir_all(&root).unwrap();
        let n = 200; let t = std::time::Instant::now();
        std::thread::scope(|s| { for k in 0..threads { let root = &root; s.spawn(move || {
            for i in (k..n).step_by(threads) { let p = root.join(format!("{i}.part")); let mut f = std::fs::File::create(&p).unwrap();
                f.write_all(&vec![i as u8; 30000]).unwrap(); f.sync_all().unwrap(); std::fs::rename(&p, root.join(format!("{i}"))).unwrap(); } }); } });
        std::fs::File::open(&root).unwrap().sync_all().unwrap();
        println!("{threads} threads: {:.2} ms per blob (sync_all each, one dir sync)", t.elapsed().as_secs_f64() * 1e3 / n as f64);
    }
}
