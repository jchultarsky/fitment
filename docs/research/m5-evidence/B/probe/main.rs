//! M5 design B prototype: a SQLite level-1 index over fitment-core's
//! tokens, features.v1 stored per part, and a catalog query that judges
//! the hits from stored features alone. Measurement code, not product code.
//!
//!   probe ingest DB DIR COPIES THREADS      read DIR's accepted parts, write
//!                                           COPIES scaled copies of each
//!   probe query DB SOCKET.json THREADS OUT [mem]   level 1, level 2, judge
//!   probe roundtrip DIR MANIFEST SOCKETDIR  features.v1 round trip and
//!                                           judge_step == judge_part(stored)
//!   probe scale FILE S OUT                  one scaled copy, for checking

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fitment_core::{
    CandidateResult, CandidateSource, FeaturesDocument, IndexSource, IndexTokens, MatchDocument,
    MatchOptions, MatchPolicy, Matcher, NoFitVerifier, PartSignature, SocketDocument,
    SocketSignature, SocketView, Source, bin, query_source, sense_code, sha256_hex, shortlist,
};
use rusqlite::{Connection, OpenFlags, params};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("ingest") => ingest(&args[2..]),
        Some("query") => query(&args[2..]),
        Some("roundtrip") => roundtrip(&args[2..]),
        Some("dedup") => dedup(&args[2..]),
        Some("sigsize") => sigsize(&args[2..]),
        Some("scale") => {
            let text = fs::read_to_string(&args[2]).unwrap();
            let s: f64 = args[3].parse().unwrap();
            fs::write(&args[4], scale_step(&text, s).unwrap()).unwrap();
        }
        _ => eprintln!("usage: see the source"),
    }
}

/// A copy of a STEP file whose length unit is `s` mm: every length,
/// uncertainty included, scales by `s` exactly as a reader applies it.
fn scale_step(text: &str, s: f64) -> Option<String> {
    let needle = "( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );";
    let pos = text.find(needle)?;
    let line_start = text[..pos].rfind('\n').map_or(0, |i| i + 1);
    let id: u64 = text[line_start..pos]
        .trim()
        .trim_start_matches('#')
        .split('=')
        .next()?
        .trim()
        .parse()
        .ok()?;
    let mut max = 0_u64;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'#' {
            let mut j = i + 1;
            let mut v = 0_u64;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                v = v * 10 + u64::from(bytes[j] - b'0');
                j += 1;
            }
            max = max.max(v);
            i = j;
        } else {
            i += 1;
        }
    }
    let (a, b, c) = (max + 1, max + 2, max + 3);
    let replacement = format!(
        "#{id} = ( CONVERSION_BASED_UNIT('SCALED',#{a}) LENGTH_UNIT() NAMED_UNIT(#{b}) );\n\
         #{a} = LENGTH_MEASURE_WITH_UNIT(LENGTH_MEASURE({s:?}),#{c});\n\
         #{b} = DIMENSIONAL_EXPONENTS(1.,0.,0.,0.,0.,0.,0.);\n\
         #{c} = ( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );"
    );
    let end = pos + needle.len();
    Some(format!("{}{}{}", &text[..line_start], replacement, &text[end..]))
}

/// One part ready to store.
struct Row {
    label: String,
    sha256: String,
    bytes: u64,
    features: String,
    tokens: String,
    planes: u32,
    lever: f64,
    incomplete: bool,
    postings: Vec<(u8, i64, usize, f64)>,
    read: Duration,
}

fn row_of(label: String, step: &[u8]) -> Option<Row> {
    let t0 = Instant::now();
    let doc = FeaturesDocument::read(step);
    let read = t0.elapsed();
    let part = doc.part()?;
    let sig = PartSignature::of(part);
    let tokens = IndexTokens::of(&sig);
    let mut postings = Vec::new();
    for (k, (sense, ds)) in tokens.stacks.iter().enumerate() {
        for d in ds {
            postings.push((sense_code(*sense), bin(*d), k, *d));
        }
    }
    Some(Row {
        label,
        sha256: sha256_hex(step),
        bytes: step.len() as u64,
        features: if std::env::var_os("PROBE_PRETTY").is_some() { doc.to_json() } else { serde_json::to_string(&doc).unwrap() },
        tokens: serde_json::to_string(&tokens).unwrap(),
        planes: tokens.planes,
        lever: tokens.lever_mm,
        incomplete: tokens.pairs_incomplete,
        postings,
        read,
    })
}

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS part (
  id INTEGER PRIMARY KEY,
  label TEXT NOT NULL,
  sha256 TEXT NOT NULL UNIQUE,
  bytes INTEGER NOT NULL,
  extractor_version INTEGER NOT NULL,
  signature_version INTEGER NOT NULL,
  planes INTEGER NOT NULL,
  lever_mm REAL NOT NULL,
  pairs_incomplete INTEGER NOT NULL,
  tokens TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS features (
  part INTEGER PRIMARY KEY REFERENCES part(id) ON DELETE CASCADE,
  json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS posting (
  sense INTEGER NOT NULL,
  bin INTEGER NOT NULL,
  part INTEGER NOT NULL REFERENCES part(id) ON DELETE CASCADE,
  stack INTEGER NOT NULL,
  d REAL NOT NULL,
  PRIMARY KEY (sense, bin, part, stack, d)
) WITHOUT ROWID;
";

fn list_step(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("stp"))
        })
        .collect();
    v.sort();
    v
}

/// A deterministic scale in [0.5, 2): log-uniform from a hash of (base, k).
fn scale_of(base: usize, k: usize) -> f64 {
    let mut x = (base as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (k as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    let u = (x >> 11) as f64 / (1_u64 << 53) as f64;
    // 4^u / 2, rounded to 6 decimals so the STEP text is short.
    let s = 0.5 * (u * std::f64::consts::LN_2 * 2.0).exp();
    (s * 1e6).round() / 1e6
}

fn ingest(args: &[String]) {
    let db = Path::new(&args[0]);
    let dir = Path::new(&args[1]);
    let copies: usize = args[2].parse().unwrap();
    let threads: usize = args[3].parse().unwrap();
    let dup = args.get(4).is_some_and(|m| m == "dup");
    let t_all = Instant::now();
    // Bases: accepted single-part files.
    let mut bases: Vec<(String, String)> = Vec::new();
    for p in list_step(dir) {
        let text = match fs::read_to_string(&p) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let doc = FeaturesDocument::read(text.as_bytes());
        if doc.part().is_some() {
            bases.push((p.file_name().unwrap().to_string_lossy().into_owned(), text));
        }
    }
    eprintln!("bases: {} accepted parts", bases.len());
    let mut conn = Connection::open(db).unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    let jobs: Vec<(usize, usize)> = (0..copies)
        .flat_map(|k| (0..bases.len()).map(move |b| (b, k)))
        .collect();
    let (tx, rx) = mpsc::sync_channel::<Row>(4096);
    let mut read_total = Duration::ZERO;
    let mut stored = 0_usize;
    let mut postings_n = 0_usize;
    let mut refused = 0_usize;
    let t_ingest = Instant::now();
    std::thread::scope(|scope| {
        for t in 0..threads {
            let tx = tx.clone();
            let jobs = &jobs;
            let bases = &bases;
            scope.spawn(move || {
                for (b, k) in jobs.iter().skip(t).step_by(threads) {
                    let (name, text) = &bases[*b];
                    let (label, step) = if *k == 0 {
                        (name.clone(), text.clone())
                    } else if dup {
                        (format!("{name}#{k}"), text.replacen("FILE_NAME('", &format!("FILE_NAME('copy{k}_"), 1))
                    } else {
                        let s = scale_of(*b, *k);
                        match scale_step(text, s) {
                            Some(x) => (format!("{name}@{s}"), x),
                            None => continue,
                        }
                    };
                    match row_of(label, step.as_bytes()) {
                        Some(r) => {
                            if tx.send(r).is_err() {
                                return;
                            }
                        }
                        None => eprintln!("refused: base {b} copy {k}"),
                    }
                }
            });
        }
        drop(tx);
        let mut txn = conn.transaction().unwrap();
        let mut in_txn = 0;
        for r in rx {
            read_total += r.read;
            {
                let mut ins = txn
                    .prepare_cached(
                        "INSERT OR IGNORE INTO part (label, sha256, bytes, extractor_version, signature_version, planes, lever_mm, pairs_incomplete, tokens) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    )
                    .unwrap();
                let n = ins
                    .execute(params![
                        r.label,
                        r.sha256,
                        r.bytes as i64,
                        fitment_core::EXTRACTOR_VERSION,
                        fitment_core::SIGNATURE_VERSION,
                        r.planes,
                        r.lever,
                        r.incomplete,
                        r.tokens
                    ])
                    .unwrap();
                if n == 0 {
                    refused += 1; // a duplicate SHA-256
                    continue;
                }
            }
            let id = txn.last_insert_rowid();
            txn.prepare_cached("INSERT INTO features (part, json) VALUES (?1, ?2)")
                .unwrap()
                .execute(params![id, r.features])
                .unwrap();
            {
                let mut p = txn
                    .prepare_cached("INSERT OR IGNORE INTO posting (sense, bin, part, stack, d) VALUES (?1,?2,?3,?4,?5)")
                    .unwrap();
                for (s, b, k, d) in &r.postings {
                    p.execute(params![s, b, id, *k as i64, d]).unwrap();
                    postings_n += 1;
                }
            }
            stored += 1;
            in_txn += 1;
            if in_txn == 5000 {
                txn.commit().unwrap();
                txn = conn.transaction().unwrap();
                in_txn = 0;
            }
        }
        txn.commit().unwrap();
    });
    let wall = t_ingest.elapsed();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); ANALYZE;").unwrap();
    let size = fs::metadata(db).map(|m| m.len()).unwrap_or(0);
    let fsize: i64 = conn
        .query_row("SELECT SUM(LENGTH(json)) FROM features", [], |r| r.get(0))
        .unwrap_or(0);
    let tsize: i64 = conn
        .query_row("SELECT SUM(LENGTH(tokens)) FROM part", [], |r| r.get(0))
        .unwrap_or(0);
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM part", [], |r| r.get(0)).unwrap();
    println!(
        "ingest: {stored} stored ({refused} duplicate), {total} in catalog, {postings_n} postings; wall {:.2} s with {threads} threads; read {:.2} s cpu ({:.3} ms/part); db {:.1} MB (features JSON {:.1} MB, tokens JSON {:.1} MB); total {:.2} s",
        wall.as_secs_f64(),
        read_total.as_secs_f64(),
        read_total.as_secs_f64() * 1e3 / stored.max(1) as f64,
        size as f64 / 1e6,
        fsize as f64 / 1e6,
        tsize as f64 / 1e6,
        t_all.elapsed().as_secs_f64()
    );
}

/// Level 1's postings in SQLite.
struct Sql<'c> {
    conn: &'c Connection,
}

impl IndexSource for Sql<'_> {
    type Error = rusqlite::Error;
    fn postings(
        &self,
        sense: fitment_core::Sense,
        lo: i64,
        hi: i64,
        out: &mut dyn FnMut(u64, usize, f64),
    ) -> Result<(), Self::Error> {
        let mut st = self
            .conn
            .prepare_cached("SELECT part, stack, d FROM posting WHERE sense = ?1 AND bin BETWEEN ?2 AND ?3")?;
        let mut rows = st.query(params![sense_code(sense), lo, hi])?;
        while let Some(r) = rows.next()? {
            let id: i64 = r.get(0)?;
            let k: i64 = r.get(1)?;
            let d: f64 = r.get(2)?;
            out(id as u64, k as usize, d);
        }
        Ok(())
    }
    fn all(&self) -> Result<Vec<u64>, Self::Error> {
        let mut st = self.conn.prepare_cached("SELECT id FROM part ORDER BY id")?;
        let ids = st
            .query_map([], |r| r.get::<_, i64>(0))?
            .map(|x| x.map(|v| v as u64))
            .collect();
        ids
    }
    fn tokens(&self, ids: &[u64], out: &mut dyn FnMut(u64, &IndexTokens)) -> Result<(), Self::Error> {
        let mut st = self.conn.prepare_cached("SELECT tokens FROM part WHERE id = ?1")?;
        for id in ids {
            let text: String = st.query_row(params![*id as i64], |r| r.get(0))?;
            let t: IndexTokens = serde_json::from_str(&text).expect("tokens");
            out(*id, &t);
        }
        Ok(())
    }
    fn max_lever(&self) -> Result<f64, Self::Error> {
        self.conn
            .query_row("SELECT COALESCE(MAX(lever_mm), 0.0) FROM part", [], |r| r.get(0))
    }
}

#[derive(Default, Clone, Copy)]
struct Times {
    load: Duration,
    parse: Duration,
    level2: Duration,
    judge: Duration,
    kept: usize,
    dropped: usize,
}

fn query(args: &[String]) {
    let db = Path::new(&args[0]);
    let socket_text = fs::read_to_string(&args[1]).unwrap();
    let threads: usize = args[2].parse().unwrap();
    let out = &args[3];
    let mem = args.get(4).is_some_and(|m| m == "mem");
    let every = args.get(4).is_some_and(|m| m == "all");
    let t_all = Instant::now();
    let document = SocketDocument::from_json(&socket_text).expect("socket document");
    let policy = MatchPolicy::default();
    let options = MatchOptions::new(false, false);
    let matcher = Matcher::new(&document, policy.clone(), options, &NoFitVerifier).expect("matcher");
    let ids: Vec<String> = document
        .extracted()
        .map(|e| e.sockets.iter().map(|s| s.id.clone()).collect())
        .unwrap_or_default();
    let sigs: Vec<SocketSignature> = ids
        .iter()
        .filter_map(|id| SocketView::new(&document, id))
        .map(|v| SocketSignature::of(&v, &policy))
        .collect();
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM part", [], |r| r.get(0)).unwrap();
    // Level 1.
    let t1 = Instant::now();
    let mut hits: Option<Vec<u64>> = None;
    let mut blocked = None;
    let mem_index;
    let mut build = Duration::ZERO;
    if mem {
        let tb = Instant::now();
        let mut idx = fitment_core::ShortlistIndex::new();
        let mut st = conn.prepare("SELECT id, tokens FROM part ORDER BY id").unwrap();
        let mut rows = st.query([]).unwrap();
        while let Some(r) = rows.next().unwrap() {
            let id: i64 = r.get(0).unwrap();
            let text: String = r.get(1).unwrap();
            let t: IndexTokens = serde_json::from_str(&text).unwrap();
            idx.insert(id as u64, &t);
        }
        build = tb.elapsed();
        mem_index = Some(idx);
    } else {
        mem_index = None;
    }
    for sig in &sigs {
        let got = match &mem_index {
            Some(idx) => idx.query(sig).map_err(|b| b),
            None => query_source(sig, &Sql { conn: &conn }).unwrap(),
        };
        match got {
            Err(b) => {
                blocked = Some(b);
                break;
            }
            Ok(v) => {
                hits = Some(match hits.take() {
                    None => v,
                    Some(h) => h.into_iter().filter(|id| v.binary_search(id).is_ok()).collect(),
                });
            }
        }
    }
    let level1 = t1.elapsed();
    if let Some(b) = blocked {
        println!("blocked: {b:?}; catalog {total}");
        return;
    }
    let mut hits = hits.unwrap_or_default();
    if every {
        hits = Sql { conn: &conn }.all().unwrap();
    }
    // Level 2 and judgment, over threads.
    let t2 = Instant::now();
    let (tx, rx) = mpsc::channel::<(u64, Option<CandidateResult>, Times)>();
    std::thread::scope(|scope| {
        for t in 0..threads {
            let tx = tx.clone();
            let hits = &hits;
            let document = &document;
            let sigs = &sigs;
            let policy = policy.clone();
            scope.spawn(move || {
                let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
                let matcher = Matcher::new(document, policy, options, &NoFitVerifier).unwrap();
                let mut st = conn
                    .prepare("SELECT p.sha256, p.bytes, f.json FROM part p JOIN features f ON f.part = p.id WHERE p.id = ?1")
                    .unwrap();
                for id in hits.iter().skip(t).step_by(threads) {
                    let mut times = Times::default();
                    let a = Instant::now();
                    let (sha, bytes, json): (String, i64, String) = st
                        .query_row(params![*id as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                        .unwrap();
                    times.load = a.elapsed();
                    let a = Instant::now();
                    let doc = FeaturesDocument::from_json(&json).expect("stored features");
                    times.parse = a.elapsed();
                    let part = doc.part().expect("accepted");
                    let a = Instant::now();
                    let ps = PartSignature::of(part);
                    let kept = sigs.iter().all(|s| shortlist(s, &ps).is_ok());
                    times.level2 = a.elapsed();
                    if !kept {
                        times.dropped = 1;
                        tx.send((*id, None, times)).unwrap();
                        continue;
                    }
                    times.kept = 1;
                    let source: Source = serde_json::from_value(serde_json::json!({
                        "sha256": sha, "bytes": bytes, "header": doc.header()
                    }))
                    .unwrap();
                    let a = Instant::now();
                    let r = matcher.judge_part(part, CandidateSource::File(source));
                    times.judge = a.elapsed();
                    tx.send((*id, Some(r), times)).unwrap();
                }
            });
        }
        drop(tx);
    });
    let mut sum = Times::default();
    let mut results = Vec::new();
    let mut slowest = (Duration::ZERO, 0_u64);
    for (id, r, t) in rx {
        sum.load += t.load;
        sum.parse += t.parse;
        sum.level2 += t.level2;
        sum.judge += t.judge;
        sum.kept += t.kept;
        sum.dropped += t.dropped;
        let one = t.load + t.parse + t.level2 + t.judge;
        if one > slowest.0 {
            slowest = (one, id);
        }
        if let Some(r) = r {
            results.push((r, vec![format!("catalog:{id}")]));
        }
    }
    let judged_wall = t2.elapsed();
    let t3 = Instant::now();
    let doc = MatchDocument::new(&matcher, results);
    let json = doc.to_json();
    fs::write(out, &json).unwrap();
    let s = doc.summary();
    println!(
        "catalog {total}; level1 {} hits in {:.2} ms{}; level2 kept {} dropped {}; wall judge {:.1} ms ({threads} threads): cpu load {:.1} ms parse {:.1} ms level2 {:.1} ms judge {:.1} ms; slowest part {:.1} ms (id {}); document {:.1} ms; {:?}; total {:.1} ms",
        hits.len(),
        level1.as_secs_f64() * 1e3,
        if mem { format!(" (index built in {:.1} ms)", build.as_secs_f64() * 1e3) } else { String::new() },
        sum.kept,
        sum.dropped,
        judged_wall.as_secs_f64() * 1e3,
        sum.load.as_secs_f64() * 1e3,
        sum.parse.as_secs_f64() * 1e3,
        sum.level2.as_secs_f64() * 1e3,
        sum.judge.as_secs_f64() * 1e3,
        slowest.0.as_secs_f64() * 1e3,
        slowest.1,
        t3.elapsed().as_secs_f64() * 1e3,
        (s.candidates, s.interface_match, s.reject),
        t_all.elapsed().as_secs_f64() * 1e3
    );
}

/// features.v1 round trip and the judgment from stored features.
fn roundtrip(args: &[String]) {
    let dir = Path::new(&args[0]);
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&args[1]).unwrap()).unwrap();
    let sockets = Path::new(&args[2]);
    let (mut same, mut differ, mut refused) = (0, 0, 0);
    let mut sizes = Vec::new();
    let mut times = (Duration::ZERO, Duration::ZERO);
    let mut times2 = Duration::ZERO;
    let (mut csizes, mut psizes) = (0_usize, 0_usize);
    for p in list_step(dir) {
        let bytes = fs::read(&p).unwrap();
        let a = Instant::now();
        let doc = FeaturesDocument::read(&bytes);
        times.0 += a.elapsed();
        if doc.part().is_none() {
            refused += 1;
            continue;
        }
        let json = doc.to_json();
        sizes.push(json.len());
        let a = Instant::now();
        let back = FeaturesDocument::from_json(&json).expect("reads back");
        times.1 += a.elapsed();
        let compact = serde_json::to_string(&doc).unwrap();
        let a = Instant::now();
        let back2 = FeaturesDocument::from_json(&compact).expect("compact reads back");
        times2 += a.elapsed();
        csizes += compact.len();
        psizes += json.len();
        if back == doc && back.to_json() == json && back2 == doc && back2.to_json() == json {
            same += 1;
        } else {
            differ += 1;
            println!("DIFFERS: {}", p.display());
        }
    }
    sizes.sort_unstable();
    let n = sizes.len();
    println!(
        "round trip: {same} equal, {differ} differ, {refused} refused; features JSON median {} B, p90 {} B, max {} B; read {:.2} ms/part, from_json {:.3} ms/part",
        sizes[n / 2],
        sizes[n * 9 / 10],
        sizes[n - 1],
        times.0.as_secs_f64() * 1e3 / n as f64,
        times.1.as_secs_f64() * 1e3 / n as f64
    );
    println!("compact JSON: {:.3} of pretty's bytes; from_json {:.3} ms/part (pretty {:.3})", csizes as f64 / psizes as f64, times2.as_secs_f64() * 1e3 / n as f64, times.1.as_secs_f64() * 1e3 / n as f64);
    // Every match case with a file candidate: judge_step on the bytes
    // against judge_part on the part read back from stored JSON.
    let mut by_socket: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for c in manifest["cases"].as_array().unwrap() {
        if c["kind"] != "match" {
            continue;
        }
        let Some(file) = c["candidate"]["file"].as_str() else { continue };
        let s = &c["socket"];
        let key = format!(
            "{}__{}",
            s["case"].as_str().unwrap(),
            s["part"].as_str().unwrap()
        );
        by_socket.entry(key).or_default().push(file.to_owned());
    }
    let (mut eq, mut ne, mut missing) = (0, 0, 0);
    for (key, files) in &by_socket {
        let path = sockets.join(format!("{key}.json"));
        let Ok(text) = fs::read_to_string(&path) else {
            missing += 1;
            continue;
        };
        let Ok(document) = SocketDocument::from_json(&text) else {
            missing += 1;
            continue;
        };
        let Ok(matcher) =
            Matcher::new(&document, MatchPolicy::default(), MatchOptions::new(false, false), &NoFitVerifier)
        else {
            missing += 1;
            continue;
        };
        for f in files {
            let bytes = fs::read(dir.join(f)).unwrap();
            let direct = matcher.judge_step(&bytes);
            let doc = FeaturesDocument::read(&bytes);
            let Some(part) = doc.part() else {
                // refused: the catalog quarantines it; nothing to compare.
                continue;
            };
            let back = FeaturesDocument::from_json(&doc.to_json()).unwrap();
            let source: Source = serde_json::from_value(serde_json::json!({
                "sha256": sha256_hex(&bytes), "bytes": bytes.len(), "header": doc.header()
            }))
            .unwrap();
            let stored = matcher.judge_part(back.part().unwrap(), CandidateSource::File(source));
            let _ = part;
            if stored == direct {
                eq += 1;
            } else {
                ne += 1;
                println!("JUDGE DIFFERS: {key} {f}");
            }
        }
    }
    println!("judge_step == judge_part(stored): {eq} equal, {ne} differ; sockets missing {missing}");
}


/// The query with judgment once per distinct Part (features digest): the
/// same document as `query`, by construction checked in the caller.
fn dedup(args: &[String]) {
    let db = Path::new(&args[0]);
    let socket_text = fs::read_to_string(&args[1]).unwrap();
    let threads: usize = args[2].parse().unwrap();
    let out = &args[3];
    let t_all = Instant::now();
    let document = SocketDocument::from_json(&socket_text).expect("socket document");
    let policy = MatchPolicy::default();
    let options = MatchOptions::new(false, false);
    let matcher = Matcher::new(&document, policy.clone(), options, &NoFitVerifier).unwrap();
    let ids: Vec<String> = document.extracted().map(|e| e.sockets.iter().map(|s| s.id.clone()).collect()).unwrap_or_default();
    let sigs: Vec<SocketSignature> = ids.iter().filter_map(|id| SocketView::new(&document, id)).map(|v| SocketSignature::of(&v, &policy)).collect();
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let mut hits: Option<Vec<u64>> = None;
    for sig in &sigs {
        match query_source(sig, &Sql { conn: &conn }).unwrap() {
            Err(b) => { println!("blocked: {b:?}"); return; }
            Ok(v) => hits = Some(match hits.take() { None => v, Some(h) => h.into_iter().filter(|id| v.binary_search(id).is_ok()).collect() }),
        }
    }
    let hits = hits.unwrap_or_default();
    // Phase A: level 2 and the digest of each kept part.
    let ta = Instant::now();
    let (tx, rx) = mpsc::channel::<(u64, String, i64, String, String)>();
    std::thread::scope(|scope| {
        for t in 0..threads {
            let tx = tx.clone();
            let hits = &hits;
            let sigs = &sigs;
            scope.spawn(move || {
                let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
                let mut st = conn.prepare("SELECT p.sha256, p.bytes, f.json FROM part p JOIN features f ON f.part = p.id WHERE p.id = ?1").unwrap();
                for id in hits.iter().skip(t).step_by(threads) {
                    let (sha, bytes, json): (String, i64, String) = st.query_row(params![*id as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
                    let doc = FeaturesDocument::from_json(&json).unwrap();
                    let part = doc.part().unwrap();
                    let ps = PartSignature::of(part);
                    if !sigs.iter().all(|s| shortlist(s, &ps).is_ok()) { continue; }
                    let part_json = serde_json::to_string(part).unwrap();
                    let digest = sha256_hex(part_json.as_bytes());
                    let header = serde_json::to_string(&doc.header()).unwrap();
                    tx.send((*id, sha, bytes, header, format!("{digest}\n{part_json}"))).unwrap();
                }
            });
        }
        drop(tx);
    });
    let mut kept: Vec<(u64, String, i64, String, String, String)> = rx.into_iter().map(|(id, sha, b, h, dj)| {
        let (d, j) = dj.split_once('\n').unwrap();
        (id, sha, b, h, d.to_owned(), j.to_owned())
    }).collect();
    kept.sort_by_key(|k| k.0);
    let phase_a = ta.elapsed();
    // Phase B: one judgment per digest.
    let tb = Instant::now();
    let mut reps: BTreeMap<String, usize> = BTreeMap::new();
    for (k, row) in kept.iter().enumerate() { reps.entry(row.4.clone()).or_insert(k); }
    let rep_list: Vec<usize> = reps.values().copied().collect();
    let (tx, rx) = mpsc::channel::<(String, CandidateResult)>();
    std::thread::scope(|scope| {
        for t in 0..threads {
            let tx = tx.clone();
            let rep_list = &rep_list;
            let kept = &kept;
            let document = &document;
            let policy = policy.clone();
            scope.spawn(move || {
                let matcher = Matcher::new(document, policy, options, &NoFitVerifier).unwrap();
                for k in rep_list.iter().skip(t).step_by(threads) {
                    let row = &kept[*k];
                    let part: fitment_core::Part = serde_json::from_str(&row.5).unwrap();
                    let r = matcher.judge_part(&part, CandidateSource::Given);
                    tx.send((row.4.clone(), r)).unwrap();
                }
            });
        }
        drop(tx);
    });
    let judged: BTreeMap<String, CandidateResult> = rx.into_iter().collect();
    let phase_b = tb.elapsed();
    let results: Vec<(CandidateResult, Vec<String>)> = kept.iter().map(|row| {
        let mut r = judged[&row.4].clone();
        let header: serde_json::Value = serde_json::from_str(&row.3).unwrap();
        r.source = CandidateSource::File(serde_json::from_value(serde_json::json!({"sha256": row.1, "bytes": row.2, "header": header})).unwrap());
        (r, vec![format!("catalog:{}", row.0)])
    }).collect();
    let doc = MatchDocument::new(&matcher, results);
    fs::write(out, doc.to_json()).unwrap();
    println!("dedup: level1 {} hits, kept {}, distinct parts {}; phase A {:.1} ms, judge {:.1} ms; total {:.1} ms", hits.len(), kept.len(), rep_list.len(), phase_a.as_secs_f64()*1e3, phase_b.as_secs_f64()*1e3, t_all.elapsed().as_secs_f64()*1e3);
}


/// Sizes of what could be stored per part: compact features, signature, tokens.
fn sigsize(args: &[String]) {
    let (mut f, mut s, mut t, mut n) = (0_usize, 0_usize, 0_usize, 0_usize);
    let (mut tf, mut ts) = (Duration::ZERO, Duration::ZERO);
    for p in list_step(Path::new(&args[0])) {
        let bytes = fs::read(&p).unwrap();
        let doc = FeaturesDocument::read(&bytes);
        let Some(part) = doc.part() else { continue };
        let fj = serde_json::to_string(&doc).unwrap();
        let sig = PartSignature::of(part);
        let sj = serde_json::to_string(&sig).unwrap();
        let tj = serde_json::to_string(&IndexTokens::of(&sig)).unwrap();
        let a = Instant::now();
        let _ = FeaturesDocument::from_json(&fj).unwrap();
        tf += a.elapsed();
        let a = Instant::now();
        let _: serde_json::Value = serde_json::from_str(&sj).unwrap();
        ts += a.elapsed();
        f += fj.len(); s += sj.len(); t += tj.len(); n += 1;
    }
    println!("{n} parts: compact features {:.1} KB/part (from_json {:.3} ms), signature {:.1} KB/part (parse as Value {:.3} ms), tokens {:.1} KB/part",
        f as f64 / n as f64 / 1e3, tf.as_secs_f64() * 1e3 / n as f64, s as f64 / n as f64 / 1e3, ts.as_secs_f64() * 1e3 / n as f64, t as f64 / n as f64 / 1e3);
}
