//! Gives micropanel.exe its icon and version details: writes a Windows resource file (.res) and hands
//! it to the linker, so no resource compiler is needed.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" || var("CARGO_CFG_TARGET_ENV") != "msvc" { return; }
    let mut res = entry(0, 0, &[]); // a .res file starts with an empty entry
    let sizes = [16u32, 20, 24, 32, 48, 64, 256];
    for (i, n) in sizes.iter().enumerate() { res.extend(entry(3, i as u16 + 1, &bitmap(*n))); }
    res.extend(entry(14, 1, &group(&sizes)));
    res.extend(entry(16, 1, &version(&var("CARGO_PKG_VERSION"))));
    let out = PathBuf::from(var("OUT_DIR")).join("app.res");
    std::fs::write(&out, res).expect("can't write the resource file");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

fn w16(v: &mut Vec<u8>, x: u16) { v.extend(x.to_le_bytes()); }
fn w32(v: &mut Vec<u8>, x: u32) { v.extend(x.to_le_bytes()); }
fn pad(v: &mut Vec<u8>) { while v.len() % 4 != 0 { v.push(0); } }

/// RESOURCEHEADER + data; type and name are numbers.
fn entry(kind: u16, name: u16, data: &[u8]) -> Vec<u8> {
    let mut v = vec![];
    w32(&mut v, data.len() as u32);
    w32(&mut v, 32);
    for x in [0xFFFF, kind, 0xFFFF, name] { w16(&mut v, x); }
    w32(&mut v, 0); // data version
    w16(&mut v, if kind == 0 { 0 } else { 0x1010 }); // moveable, discardable
    w16(&mut v, if kind == 0 { 0 } else { 0x0409 }); // English
    w32(&mut v, 0);
    w32(&mut v, 0);
    v.extend(data);
    pad(&mut v);
    v
}

/// The tray's picture, a dark disc with the three balls: colour (BGRA) of a point, 0..1 from the top left.
fn colour(x: f32, y: f32) -> [f32; 4] {
    let d = |cx: f32, cy: f32| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    if d(0.25, 0.5625).min(d(0.5, 0.5625)).min(d(0.75, 0.5625)) < 0.106 { return [235.0, 235.0, 235.0, 255.0]; }
    if (x - 0.5).abs() < 0.28 && (y - 0.266).abs() < 0.047 { return [120.0, 200.0, 120.0, 255.0]; }
    if d(0.5, 0.5) < 0.484 { [52.0, 44.0, 40.0, 255.0] } else { [0.0; 4] }
}

/// BITMAPINFOHEADER, 32-bit pixels bottom up, empty mask.
fn bitmap(n: u32) -> Vec<u8> {
    let mut v = vec![];
    w32(&mut v, 40);
    w32(&mut v, n);
    w32(&mut v, 2 * n);
    w16(&mut v, 1);
    w16(&mut v, 32);
    for _ in 0..6 { w32(&mut v, 0); }
    const S: u32 = 8;
    for y in (0..n).rev() {
        for x in 0..n {
            let mut sum = [0.0f32; 4];
            for k in 0..S * S {
                let p = |c: u32, s: u32| (c as f32 + (s as f32 + 0.5) / S as f32) / n as f32;
                let c = colour(p(x, k % S), p(y, k / S));
                for i in 0..3 { sum[i] += c[i] * c[3]; }
                sum[3] += c[3];
            }
            for i in 0..3 { v.push(if sum[3] > 0.0 { (sum[i] / sum[3]).round() as u8 } else { 0 }); }
            v.push((sum[3] / (S * S) as f32).round() as u8);
        }
    }
    v.resize(v.len() + (n.div_ceil(32) * 4 * n) as usize, 0);
    v
}

fn group(sizes: &[u32]) -> Vec<u8> {
    let mut v = vec![];
    for x in [0, 1, sizes.len() as u16] { w16(&mut v, x); }
    for (i, n) in sizes.iter().enumerate() {
        v.extend([*n as u8, *n as u8, 0, 0]); // 256 is written as 0
        w16(&mut v, 1);
        w16(&mut v, 32);
        w32(&mut v, bitmap(*n).len() as u32);
        w16(&mut v, i as u16 + 1);
    }
    v
}

/// One block of VS_VERSIONINFO: length, value length, type, key, value, children.
fn block(key: &str, text: bool, value: &[u8], value_len: usize, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    w16(&mut v, value_len as u16);
    w16(&mut v, text as u16);
    for c in key.encode_utf16().chain([0]) { w16(&mut v, c); }
    pad(&mut v);
    v.extend(value);
    for c in children { pad(&mut v); v.extend(c); }
    let n = v.len() as u16;
    v[..2].copy_from_slice(&n.to_le_bytes());
    v
}

fn version(ver: &str) -> Vec<u8> {
    let mut n = ver.split(['.', '-']).map(|x| x.parse::<u32>().unwrap_or(0));
    let mut next = || n.next().unwrap_or(0);
    let (hi, lo) = ((next() << 16) | next(), next() << 16);
    let mut f = vec![];
    for x in [0xFEEF04BD, 0x10000, hi, lo, hi, lo, 0x3F, 0, 0x40004, 1, 0, 0, 0] { w32(&mut f, x); }
    let s = |k: &str, t: &str| {
        let u: Vec<u16> = t.encode_utf16().chain([0]).collect();
        let b: Vec<u8> = u.iter().flat_map(|c| c.to_le_bytes()).collect();
        block(k, true, &b, u.len(), &[])
    };
    let strings = block("040904B0", true, &[], 0, &[
        s("FileDescription", "Micro Color Panel helper"), s("ProductName", "Micro Color Panel helper"),
        s("FileVersion", ver), s("ProductVersion", ver), s("OriginalFilename", "micropanel.exe"),
        s("LegalCopyright", "CC0 1.0, no rights reserved"),
        s("Comments", "Not affiliated with Blackmagic Design"),
    ]);
    let lang = block("Translation", false, &[9, 4, 0xB0, 4], 4, &[]);
    block("VS_VERSION_INFO", false, &f, f.len(), &[block("StringFileInfo", true, &[], 0, &[strings]), block("VarFileInfo", true, &[], 0, &[lang])])
}
