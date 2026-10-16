use fitment_core::*;
use std::time::Instant;
fn main() {
    let files: Vec<String> = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap().lines().map(String::from).collect();
    let (mut n, mut eq, mut tread, mut tparse, mut tview, mut tsig, mut tok_bytes, mut doc_bytes) =
        (0usize, 0usize, 0f64, 0f64, 0f64, 0f64, 0usize, 0usize);
    let mut worst: Vec<(f64, f64, usize, String)> = vec![];
    for f in &files {
        let Ok(bytes) = std::fs::read(f) else { println!("unreadable {f}"); continue };
        let t = Instant::now();
        let doc = FeaturesDocument::read(&bytes);
        let dr = t.elapsed().as_secs_f64();
        let Some(_) = doc.part() else { continue };
        n += 1;
        let json = doc.to_json();
        doc_bytes += json.len();
        let t = Instant::now();
        let back = FeaturesDocument::from_json(&json).expect("readback");
        let dp = t.elapsed().as_secs_f64();
        if back == doc && back.to_json() == json { eq += 1 } else { println!("MISMATCH {f}") }
        let part = back.part().unwrap();
        let t = Instant::now();
        let view = CandidateView::of(part);
        let dv = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let sig = PartSignature::of(part);
        let tok = IndexTokens::of(&sig);
        let ds = t.elapsed().as_secs_f64();
        let _ = view;
        tok_bytes += serde_json::to_string(&tok).unwrap().len();
        tread += dr; tparse += dp; tview += dv; tsig += ds;
        worst.push((dr, dp, json.len(), f.rsplit('/').next().unwrap().to_string()));
    }
    println!("accepted {n}, roundtrip-identical {eq}");
    println!("total ms: read(STEP->doc) {:.1}  from_json {:.1}  CandidateView::of {:.1}  sig+tokens {:.1}", tread*1e3, tparse*1e3, tview*1e3, tsig*1e3);
    println!("bytes: features docs {doc_bytes}  tokens json {tok_bytes} (mean {} per part)", tok_bytes / n.max(1));
    worst.sort_by(|a, b| b.1.total_cmp(&a.1));
    for w in worst.iter().take(5) { println!("slowest from_json: read {:.1} ms, from_json {:.1} ms, {} bytes, {}", w.0*1e3, w.1*1e3, w.2, w.3); }
}
