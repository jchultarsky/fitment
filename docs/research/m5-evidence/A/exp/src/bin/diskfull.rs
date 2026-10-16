use rusqlite::{Connection, params, TransactionBehavior};
use std::io::Write;
fn main() {
    let root = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let db = root.join("index.db");
    let mut c = Connection::open(&db).unwrap();
    c.pragma_update(None, "journal_mode", "WAL").unwrap();
    c.execute_batch("CREATE TABLE IF NOT EXISTS part(sha TEXT PRIMARY KEY, doc BLOB NOT NULL) STRICT;").unwrap();
    std::fs::create_dir_all(root.join("tmp")).unwrap();
    let (mut blobs_ok, mut rows_ok) = (0, 0);
    let mut blob_err = None; let mut row_err = None;
    for i in 0..10_000 {
        let tmp = root.join("tmp").join(format!("{i}.part"));
        let r = (|| -> std::io::Result<()> { let mut f = std::fs::File::create(&tmp)?; f.write_all(&vec![7u8; 400_000])?; f.sync_all()?; Ok(()) })();
        match r { Ok(()) => { std::fs::rename(&tmp, root.join(format!("b{i}"))).unwrap(); blobs_ok += 1; }
                  Err(e) => { let len = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0); let _ = std::fs::remove_file(&tmp); blob_err = Some(format!("{e} (tmp held {len} bytes, removed)")); break; } }
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        match tx.execute("INSERT INTO part VALUES(?1, zeroblob(300000))", params![format!("{i}")]).and_then(|_| tx.commit()) {
            Ok(()) => rows_ok += 1, Err(e) => { row_err = Some(e.to_string()); break; } }
    }
    if row_err.is_none() { for j in 0..10_000 { let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).unwrap();
        match tx.execute("INSERT INTO part VALUES(?1, zeroblob(300000))", params![format!("r{j}-{}", std::process::id())]).and_then(|_| tx.commit()) {
            Ok(()) => rows_ok += 1, Err(e) => { row_err = Some(e.to_string()); break; } } } }
    let ic: String = Connection::open(&db).unwrap().query_row("PRAGMA integrity_check", [], |r| r.get(0)).unwrap();
    let n: i64 = Connection::open(&db).unwrap().query_row("SELECT count(*) FROM part", [], |r| r.get(0)).unwrap();
    println!("blobs written {blobs_ok}, rows committed {rows_ok}; blob error: {blob_err:?}; row error: {row_err:?}; after: rows {n}, integrity {ic}");
}
