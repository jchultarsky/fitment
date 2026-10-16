// For every corpus match case: judge_step(bytes) == judge_part(from_json(to_json(read(bytes))))
use fitment_core::*;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (manifest, corpus, sockets) = (&a[1], &a[2], &a[3]);
    let m: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(manifest).unwrap()).unwrap();
    let fit = NoFitVerifier;
    let (mut n, mut same, mut docsame) = (0, 0, 0);
    for c in m["cases"].as_array().unwrap().iter().filter(|c| c["kind"] == "match") {
        let sock = format!("{}/{}__{}.json", sockets, c["socket"]["case"].as_str().unwrap(), c["socket"]["part"].as_str().unwrap());
        let sdoc = SocketDocument::from_json(&std::fs::read_to_string(&sock).unwrap()).unwrap();
        let matcher = Matcher::new(&sdoc, MatchPolicy::default(), MatchOptions::default(), &fit).unwrap();
        let bytes = std::fs::read(format!("{}/{}", corpus, c["candidate"]["file"].as_str().unwrap())).unwrap();
        let r1 = matcher.judge_step(&bytes);
        let doc = FeaturesDocument::read(&bytes);
        let back = FeaturesDocument::from_json(&doc.to_json()).unwrap();
        // what the catalog would store: sha256, size, header
        let src: Source = serde_json::from_value(serde_json::json!({
            "sha256": sha256_hex(&bytes), "bytes": bytes.len(), "header": back.header() })).unwrap();
        let r2 = match back.part() {
            Some(p) => matcher.judge_part(p, CandidateSource::File(src)),
            None => { n += 1; same += 1; docsame += 1; continue; } // refused: catalog never judges it
        };
        n += 1;
        if r1 == r2 { same += 1 } else { println!("DIFF {}", c["id"]); }
        let d1 = MatchDocument::new(&matcher, vec![(r1, vec!["x".into()])]);
        let d2 = MatchDocument::new(&matcher, vec![(r2, vec!["y".into()])]);
        if d1.digest() == d2.digest() { docsame += 1 } else { println!("DOCDIFF {}", c["id"]); }
    }
    println!("cases {n}: CandidateResult equal {same}, document digest equal {docsame}");
}
