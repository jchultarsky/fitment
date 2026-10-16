// gc vs ingest vs delete race: blob unlinked only under the SQLite write lock after re-checking references.
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::{Path, PathBuf}, time::Duration};
fn hex(b: &[u8]) -> String { Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect() }
fn conn(root: &Path) -> Connection { let c = Connection::open(root.join("index.db")).unwrap(); c.pragma_update(None, "journal_mode", "WAL").unwrap(); c.busy_timeout(Duration::from_secs(60)).unwrap();
    c.execute_batch("CREATE TABLE IF NOT EXISTS blob(sha TEXT PRIMARY KEY, size INT NOT NULL) STRICT;").unwrap(); c }
fn bpath(root: &Path, sha: &str) -> PathBuf { root.join("blobs").join(sha) }
fn write_blob(root: &Path, sha: &str, b: &[u8], tag: u64) { let t = root.join("tmp").join(format!("{}-{tag}.part", std::process::id()));
    let mut f = fs::File::create(&t).unwrap(); f.write_all(b).unwrap(); f.sync_all().unwrap(); drop(f); let _ = fs::rename(&t, bpath(root, sha)); }
fn main() {
    let a: Vec<String> = std::env::args().collect(); let root = PathBuf::from(&a[2]);
    let unsafe_gc = std::env::var("GC_OUTSIDE_LOCK").is_ok();
    match a[1].as_str() {
        "ingest" => { let mut c = conn(&root); let mut s: u64 = a[3].parse().unwrap();
            for i in 0.. { s ^= s << 13; s ^= s >> 7; s ^= s << 17; let k = s % 40; let b = vec![k as u8; 5000 + k as usize];
                let sha = hex(&b);
                if s % 10 < 3 { let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap(); tx.execute("DELETE FROM blob WHERE sha=?1", params![sha]).unwrap(); tx.commit().unwrap(); continue; }
                write_blob(&root, &sha, &b, i);
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
                if fs::metadata(bpath(&root, &sha)).map(|m| m.len()).ok() != Some(b.len() as u64) { write_blob(&root, &sha, &b, i + 1_000_000_000); }
                tx.execute("INSERT OR IGNORE INTO blob VALUES(?1, ?2)", params![sha, b.len() as i64]).unwrap(); tx.commit().unwrap(); } }
        "gc" => { let mut c = conn(&root); loop {
                let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
                let mut doomed = vec![];
                for f in fs::read_dir(root.join("blobs")).unwrap() { let n = f.unwrap().file_name().into_string().unwrap();
                    let refd: i64 = tx.query_row("SELECT count(*) FROM blob WHERE sha=?1", params![n], |r| r.get(0)).unwrap();
                    if refd == 0 { if unsafe_gc { doomed.push(n) } else { let _ = fs::remove_file(bpath(&root, &n)); } } }
                tx.commit().unwrap();
                for n in doomed { std::thread::sleep(Duration::from_micros(300)); let _ = fs::remove_file(bpath(&root, &n)); } } }
        "run" => { let _ = fs::remove_dir_all(&root); fs::create_dir_all(root.join("blobs")).unwrap(); fs::create_dir_all(root.join("tmp")).unwrap(); drop(conn(&root));
            let secs: u64 = a[3].parse().unwrap();
            let mut kids: Vec<_> = (0..4).map(|k| std::process::Command::new(&a[0]).args(["ingest", &a[2], &(0x1234567 + k * 77).to_string()]).spawn().unwrap()).collect();
            kids.push(std::process::Command::new(&a[0]).args(["gc", &a[2]]).spawn().unwrap());
            let mut bad = 0; let mut checks = 0;
            for _ in 0..secs * 10 { std::thread::sleep(Duration::from_millis(100));
                let c = conn(&root); let rows: Vec<(String, i64)> = c.prepare("SELECT sha, size FROM blob ORDER BY sha").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(|r| r.unwrap()).collect();
                // a reader's snapshot of rows vs files: a committed row must have its file (files are only removed under the write lock)
                for (sha, size) in rows { checks += 1; match fs::read(bpath(&root, &sha)) { Ok(b) if b.len() as i64 == size && hex(&b) == sha => {}, _ => {
                    // re-check in a fresh snapshot: still referenced?
                    let still: i64 = conn(&root).query_row("SELECT count(*) FROM blob WHERE sha=?1", params![sha], |r| r.get(0)).unwrap(); if still > 0 { if fs::metadata(bpath(&root, &sha)).is_err() { bad += 1; } } } } } }
            for k in &mut kids { k.kill().unwrap(); k.wait().unwrap(); }
            println!("{} gc: {secs} s, 4 ingest/delete processes + 1 gc loop: {checks} row checks, {bad} rows found without their blob", if unsafe_gc { "unlink AFTER commit (control)" } else { "unlink under write lock" }); }
        _ => panic!() }
}
