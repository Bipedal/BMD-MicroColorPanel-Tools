//! Decoder parity with panel.py: replays target/golden.jsonl (python tools/make_golden.py).
use micropanel::panel::Decoder;
use std::collections::HashMap;

#[test]
fn decoder_matches_panel_py() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/target/golden.jsonl");
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("SKIP: {path} missing; run python tools/make_golden.py");
        return;
    };
    let mut decs: HashMap<String, Decoder> = HashMap::new();
    let (mut n, mut nev, mut bad) = (0, 0, 0);
    for line in text.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let f = v["f"].as_str().unwrap().to_string();
        let hex = v["hex"].as_str().unwrap();
        let b: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let got: Vec<serde_json::Value> = decs.entry(f.clone()).or_default().decode(&b).iter().map(|e| e.to_json()).collect();
        let want = v["ev"].as_array().unwrap();
        n += 1;
        nev += want.len();
        if &got != want {
            bad += 1;
            if bad <= 5 { eprintln!("MISMATCH {f} {hex}\n  want {want:?}\n  got  {got:?}"); }
        }
    }
    eprintln!("golden: {n} reports, {nev} events, {bad} mismatches");
    assert!(n > 1000, "too few golden reports ({n})");
    assert_eq!(bad, 0);
}
