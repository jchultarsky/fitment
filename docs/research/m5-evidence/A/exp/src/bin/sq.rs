use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn hex(b: &[u8]) -> String { Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect() }
fn open(db: &str) -> Connection {
    let c = Connection::open(db).unwrap();
    c.pragma_update(None, "journal_mode", "WAL").unwrap();
    c.pragma_update(None, "synchronous", "FULL").unwrap();
    c.pragma_update(None, "foreign_keys", "ON").unwrap();
    c.busy_timeout(Duration::from_secs(30)).unwrap();
    c.execute_batch("CREATE TABLE IF NOT EXISTS part(sha TEXT PRIMARY KEY, size INT NOT NULL, doc BLOB NOT NULL, doc_sha TEXT NOT NULL) STRICT;").unwrap();
    c
}
fn blob_path(root: &Path, sha: &str) -> PathBuf { root.join("blobs").join(&sha[..2]).join(sha) }
fn put_blob(root: &Path, sha: &str, bytes: &[u8], sync: u8) {
    let fin = blob_path(root, sha);
    fs::create_dir_all(fin.parent().unwrap()).unwrap();
    fs::create_dir_all(root.join("tmp")).unwrap();
    let tmp = root.join("tmp").join(format!("{}-{}.part", std::process::id(), &sha[..16]));
    let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp).unwrap();
    f.write_all(bytes).unwrap();
    match sync { 0 => {}, 1 => f.sync_all().unwrap(), _ => unsafe { assert_eq!(libc::fsync(f.as_raw_fd()), 0) } }
    drop(f);
    fs::rename(&tmp, &fin).unwrap();
    if sync > 0 { let d = File::open(fin.parent().unwrap()).unwrap(); d.sync_all().unwrap(); }
}
fn ingest(c: &mut Connection, root: &Path, bytes: &[u8]) {
    let sha = hex(bytes);
    let wrong = std::env::var("ROW_FIRST").is_ok();
    if wrong { let doc = b"{}"; c.execute("INSERT OR IGNORE INTO part VALUES(?1,?2,?3,?4)", params![sha, bytes.len() as i64, &doc[..], hex(doc)]).unwrap(); put_blob(root, &sha, bytes, 1); return; }
    put_blob(root, &sha, bytes, 1);
    let doc = format!("{{\"doc-of\":\"{sha}\",\"pad\":\"{}\"}}", "x".repeat(20000));
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
    let p = blob_path(root, &sha);
    match fs::metadata(&p) { Ok(m) if m.len() == bytes.len() as u64 => {}, _ => panic!("blob vanished") }
    tx.execute("INSERT OR IGNORE INTO part VALUES(?1,?2,?3,?4)", params![sha, bytes.len() as i64, doc.as_bytes(), hex(doc.as_bytes())]).unwrap();
    tx.commit().unwrap();
}
fn check(root: &Path, db: &str) -> (usize, usize, usize) {
    let c = Connection::open(db).unwrap();
    let ic: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    assert_eq!(ic, "ok");
    let mut st = c.prepare("SELECT sha, size, doc, doc_sha FROM part ORDER BY sha").unwrap();
    let rows: Vec<(String, i64, Vec<u8>, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(|r| r.unwrap()).collect();
    for (sha, size, doc, dsha) in &rows {
        let b = fs::read(blob_path(root, sha)).expect("row without blob");
        assert_eq!(b.len() as i64, *size); assert_eq!(&hex(&b), sha, "blob corrupt"); assert_eq!(&hex(doc), dsha, "doc corrupt");
    }
    let mut blobs = 0; let mut orphans = 0;
    if let Ok(rd) = fs::read_dir(root.join("blobs")) { for d in rd { for f in fs::read_dir(d.unwrap().path()).unwrap() { blobs += 1; let n = f.unwrap().file_name().into_string().unwrap();
        let b = fs::read(blob_path(root, &n)).unwrap(); assert_eq!(hex(&b), n, "final blob name != content (partial write)");
        if !rows.iter().any(|r| r.0 == n) { orphans += 1 } } } }
    let tmps = fs::read_dir(root.join("tmp")).map(|r| r.count()).unwrap_or(0);
    let _ = blobs; (rows.len(), orphans, tmps)
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "fsync" => {
            let root = PathBuf::from(&a[2]); let n: usize = a[3].parse().unwrap(); let size: usize = a[4].parse().unwrap();
            for (mode, name) in [(0u8, "no sync"), (2, "fsync(2)+dir"), (1, "sync_all (F_FULLFSYNC)+dir")] {
                let _ = fs::remove_dir_all(&root);
                let t = Instant::now();
                for i in 0..n { let mut b = vec![b'x'; size]; b[..8].copy_from_slice(&(i as u64 ^ (mode as u64) << 40).to_le_bytes()); put_blob(&root, &hex(&b), &b, mode); }
                println!("blob write {size} B, {name}: {:.2} ms each", t.elapsed().as_secs_f64() * 1e3 / n as f64);
            }
        }
        "commit" => {
            let n: usize = a[3].parse().unwrap();
            for (sync, full) in [("NORMAL", "OFF"), ("FULL", "OFF"), ("FULL", "ON")] {
                let db = format!("{}/c-{sync}-{full}.db", a[2]); let _ = fs::remove_file(&db); let _ = fs::remove_file(format!("{db}-wal"));
                let c = open(&db); c.pragma_update(None, "synchronous", sync).unwrap(); c.pragma_update(None, "fullfsync", full).unwrap();
                let doc = vec![b'y'; 30000];
                let t = Instant::now();
                for i in 0..n { c.execute("INSERT INTO part VALUES(?1, 1, ?2, 'x')", params![format!("{i:064}"), doc]).unwrap(); }
                println!("one-row commit, WAL synchronous={sync} fullfsync={full}: {:.2} ms each", t.elapsed().as_secs_f64() * 1e3 / n as f64);
            }
        }
        "writer" => { // db id n behavior
            let c = Connection::open(&a[2]).unwrap(); c.busy_timeout(Duration::from_secs(30)).unwrap();
            let n: usize = a[4].parse().unwrap(); let mut busy = 0;
            for i in 0..n {
                let beh = if a[5] == "immediate" { TransactionBehavior::Immediate } else { TransactionBehavior::Deferred };
                let r = (|| -> rusqlite::Result<()> {
                    let mut c2 = &c; let _ = &mut c2;
                    let tx = rusqlite::Transaction::new_unchecked(&c, beh)?;
                    let _k: i64 = tx.query_row("SELECT count(*) FROM part", [], |r| r.get(0))?; // read first, then write
                    tx.execute("INSERT INTO part VALUES(?1, 1, x'00', 'x')", params![format!("{}-{i}", a[3])])?;
                    std::thread::sleep(Duration::from_micros(200));
                    tx.commit()
                })();
                if let Err(e) = r { busy += 1; if busy == 1 { eprintln!("writer {}: {e}", a[3]); } }
            }
            println!("{busy}");
        }
        "conc" => { // db behavior procs n
            let db = &a[2]; let _ = fs::remove_file(db); let _ = fs::remove_file(format!("{db}-wal")); drop(open(db));
            let procs: usize = a[4].parse().unwrap();
            let t = Instant::now();
            let ch: Vec<_> = (0..procs).map(|p| std::process::Command::new(&a[0]).args(["writer", db, &p.to_string(), &a[5], &a[3]]).output_async()).collect();
            let mut busy = 0; for c in ch { let o = c.wait_with_output().unwrap(); busy += String::from_utf8_lossy(&o.stdout).trim().parse::<usize>().unwrap_or(9999); }
            let c = Connection::open(db).unwrap(); let k: i64 = c.query_row("SELECT count(*) FROM part", [], |r| r.get(0)).unwrap();
            println!("{procs} processes x {} txns, BEGIN {}: rows {k}, failed txns {busy}, {:.0} ms", a[5], a[3], t.elapsed().as_secs_f64() * 1e3);
        }
        "snapshot" => {
            let db = format!("{}/snap.db", a[2]); let _ = fs::remove_file(&db); let _ = fs::remove_file(format!("{db}-wal"));
            let w = open(&db);
            for i in 0..100 { w.execute("INSERT INTO part VALUES(?1, 1, zeroblob(30000), 'x')", params![format!("{i:03}")]).unwrap(); }
            w.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())).unwrap();
            let r = open(&db);
            r.execute_batch("BEGIN").unwrap();
            let before: i64 = r.query_row("SELECT count(*) FROM part", [], |x| x.get(0)).unwrap();
            w.execute("DELETE FROM part WHERE sha='007'", []).unwrap();
            for i in 100..400 { w.execute("INSERT INTO part VALUES(?1, 1, zeroblob(30000), 'x')", params![format!("{i:03}")]).unwrap(); }
            let ck: (i64, i64, i64) = w.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?))).unwrap();
            let still: i64 = r.query_row("SELECT count(*) FROM part WHERE sha='007'", [], |x| x.get(0)).unwrap();
            let total: i64 = r.query_row("SELECT count(*) FROM part", [], |x| x.get(0)).unwrap();
            let wal = fs::metadata(format!("{db}-wal")).unwrap().len();
            println!("reader snapshot: before {before}, after writer deleted '007' and added 300: reader sees '007' {still}, total {total}; checkpoint(TRUNCATE) while reading -> busy={} log={} ckpt={}; WAL {:.1} MB", ck.0, ck.1, ck.2, wal as f64 / 1e6);
            r.execute_batch("COMMIT").unwrap();
            let ck: (i64, i64, i64) = w.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?))).unwrap();
            println!("after reader ends: checkpoint busy={} ; WAL {} bytes; reader now sees total {}", ck.0, fs::metadata(format!("{db}-wal")).unwrap().len(), r.query_row("SELECT count(*) FROM part", [], |x| x.get::<_, i64>(0)).unwrap());
        }
        "crashchild" => { // root: ingest forever
            let root = PathBuf::from(&a[2]); let db = root.join("index.db"); let mut c = open(db.to_str().unwrap());
            let mut i: u64 = a[3].parse().unwrap();
            loop { i += 1; let size = 1000 + (i as usize * 7919) % 200_000; let mut b = vec![b'z'; size]; b[..8].copy_from_slice(&i.to_le_bytes()); ingest(&mut c, &root, &b); }
        }
        "crash" => { // root rounds
            let root = PathBuf::from(&a[2]); let _ = fs::remove_dir_all(&root); fs::create_dir_all(&root).unwrap();
            let rounds: usize = a[3].parse().unwrap(); let db = root.join("index.db"); drop(open(db.to_str().unwrap()));
            let mut seed: u64 = 0x9e3779b97f4a7c15; let (mut maxtmp, mut maxorph) = (0, 0);
            for k in 0..rounds {
                let mut ch = std::process::Command::new(&a[0]).args(["crashchild", root.to_str().unwrap(), &(k as u64 * 1_000_000).to_string()]).spawn().unwrap();
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
                std::thread::sleep(Duration::from_micros(20_000 + seed % 150_000));
                unsafe { libc::kill(ch.id() as i32, libc::SIGKILL); } ch.wait().unwrap();
                let res = std::panic::catch_unwind(|| check(&root, db.to_str().unwrap()));
                let Ok((rows, orph, tmps)) = res else { println!("round {k}: invariant broken"); return; };
                maxtmp = maxtmp.max(tmps); maxorph = maxorph.max(orph);
                if k + 1 == rounds { println!("{rounds} kill -9 rounds: invariants held every round; final rows {rows}; max orphan blobs seen {maxorph}; max tmp files left {maxtmp}"); }
            }
        }
        "full" => {
            let db = format!("{}/full.db", a[2]); let _ = fs::remove_file(&db); let _ = fs::remove_file(format!("{db}-wal"));
            let mut c = open(&db);
            c.pragma_update(None, "max_page_count", 200).unwrap();
            let mut ok = 0; let mut err = String::new();
            for i in 0..1000 { let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
                let r = tx.execute("INSERT INTO part VALUES(?1, 1, zeroblob(30000), 'x')", params![format!("{i}")]);
                match r { Ok(_) => { tx.commit().unwrap(); ok += 1; } Err(e) => { err = e.to_string(); drop(tx); break; } } }
            let ic: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
            let n: i64 = c.query_row("SELECT count(*) FROM part", [], |r| r.get(0)).unwrap();
            println!("max_page_count=200: {ok} commits, then error '{err}'; rows after rollback {n}; integrity {ic}");
        }
        "unordered" => {
            let c = Connection::open_in_memory().unwrap();
            c.execute_batch("CREATE TABLE e(id INTEGER PRIMARY KEY, pn TEXT); INSERT INTO e(pn) VALUES('a'),('b'),('c');").unwrap();
            for rev in ["OFF", "ON"] { c.pragma_update(None, "reverse_unordered_selects", rev).unwrap();
                let v: Vec<String> = c.prepare("SELECT pn FROM e").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
                let w: Vec<String> = c.prepare("SELECT pn FROM e ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
                println!("reverse_unordered_selects={rev}: no ORDER BY {v:?}; ORDER BY id {w:?}"); }
        }
        _ => panic!(),
    }
}
trait OutputAsync { fn output_async(&mut self) -> std::process::Child; }
impl OutputAsync for std::process::Command { fn output_async(&mut self) -> std::process::Child { self.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit()).spawn().unwrap() } }
