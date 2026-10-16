use fitment_core::*;
use std::time::Instant;
fn main() {
    let files: Vec<String> = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap().lines().map(String::from).collect();
    let mut toks = vec![]; let mut sizes = vec![]; let mut docs = vec![];
    for f in &files {
        let Ok(b) = std::fs::read(f) else { continue };
        let d = FeaturesDocument::read(&b);
        if let Some(p) = d.part() { let t = IndexTokens::of(&PartSignature::of(p)); sizes.push(serde_json::to_string(&t).unwrap().len()); toks.push(t); docs.push(d.to_json()); }
    }
    sizes.sort(); let n = sizes.len();
    println!("token json bytes: median {} p90 {} max {}; pairs median {}", sizes[n/2], sizes[n*9/10], sizes[n-1], { let mut p: Vec<usize> = toks.iter().map(|t| t.pairs.len()).collect(); p.sort(); p[n/2] });
    let mut ds: Vec<usize> = docs.iter().map(|d| d.len()).collect(); ds.sort();
    println!("features doc bytes: median {} p90 {} max {}", ds[n/2], ds[n*9/10], ds[n-1]);
    for total in [10_000usize, 100_000] {
        // SQLite: store tokens json for `total` parts, then time a full load + JSON parse + index insert
        let path = format!("/private/tmp/claude-501/-Users-julian-projects-fitment/3971a646-2013-4399-8622-60a902cb28df/scratchpad/m5/A/exp/tok{total}.db");
        let _ = std::fs::remove_file(&path); let _ = std::fs::remove_file(format!("{path}-wal"));
        let c = rusqlite::Connection::open(&path).unwrap();
        c.pragma_update(None, "journal_mode", "WAL").unwrap();
        c.execute_batch("CREATE TABLE t(id INTEGER PRIMARY KEY, j BLOB NOT NULL); CREATE TABLE p(sense INT, bin INT, id INT, d REAL); ").unwrap();
        let tx = c.unchecked_transaction().unwrap();
        for i in 0..total { let t = &toks[i % n]; tx.execute("INSERT INTO t VALUES(?1, ?2)", (i as i64, serde_json::to_vec(t).unwrap())).unwrap();
            for (s, ds) in &t.stacks { for d in ds { tx.execute("INSERT INTO p VALUES(?1,?2,?3,?4)", (u8::from(*s == Sense::Shaft), (d / 0.1).floor() as i64, i as i64, *d)).unwrap(); } } }
        tx.commit().unwrap();
        c.execute_batch("CREATE INDEX pi ON p(sense, bin, id)").unwrap();
        drop(c);
        let size = std::fs::metadata(&path).unwrap().len();
        let c = rusqlite::Connection::open(&path).unwrap();
        let t0 = Instant::now();
        let mut st = c.prepare("SELECT id, j FROM t ORDER BY id").unwrap();
        let mut rows = st.query([]).unwrap();
        let mut bytes = 0usize; let mut idx = ShortlistIndex::new();
        while let Some(r) = rows.next().unwrap() { let id: i64 = r.get(0).unwrap(); let j: Vec<u8> = r.get(1).unwrap(); bytes += j.len();
            let _v: serde_json::Value = serde_json::from_slice(&j).unwrap(); idx.insert(id as u64, &toks[id as usize % n]); }
        let dt = t0.elapsed();
        // posting range query: holes of 6.6 +- 0.01, bins 64..67
        let t1 = Instant::now();
        let mut q = c.prepare("SELECT DISTINCT id FROM p WHERE sense=0 AND bin BETWEEN 64 AND 67 AND d BETWEEN 6.59 AND 6.61 ORDER BY id").unwrap();
        let hits: Vec<i64> = q.query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        println!("{total} parts: db {:.1} MB; full token load+parse+insert {:.0} ms ({:.1} MB json); one posting range query {:.2} ms -> {} ids",
            size as f64 / 1e6, dt.as_secs_f64() * 1e3, bytes as f64 / 1e6, t1.elapsed().as_secs_f64() * 1e3, hits.len());
    }
}
