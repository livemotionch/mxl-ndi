//! mxl-compare: picture accuracy of a path through the bridge. Compares the
//! v210 grains of two MXL video flows sample by sample (10-bit code values),
//! after finding the grain offset between them (the path's latency in
//! grains).
//!
//!   mxl-compare <domain A> <flow A> <domain B> <flow B> [frames] [width height]
//!
//! A is the source, B the copy (e.g. the test player and `NDI In 1 Video`).
//! Prints the offset and, for luma and chroma, the largest and mean
//! absolute error and how many samples are off by 0, 1, 2, 3–4, 5–16, more.

use std::path::Path;

use lm_mxl_ndi::convert::v210_line_bytes;
use lm_mxl_ndi::mxlio::{self, Instance};

fn unpack(g: &[u8], w: usize, h: usize, y: &mut Vec<u16>, c: &mut Vec<u16>) {
    y.clear();
    c.clear();
    let lb = v210_line_bytes(w);
    for row in 0..h {
        let r = &g[row * lb..(row + 1) * lb];
        for k in 0..2 * w {
            let word = k / 3;
            let v = u32::from_le_bytes([r[4 * word], r[4 * word + 1], r[4 * word + 2], r[4 * word + 3]]);
            let s = ((v >> (10 * (k % 3))) & 0x3FF) as u16;
            if k & 1 == 1 {
                y.push(s);
            } else {
                c.push(s);
            }
        }
    }
}

#[derive(Default)]
struct Stats {
    n: u64,
    sum: u64,
    max: u16,
    hist: [u64; 6],
}

impl Stats {
    fn add(&mut self, a: &[u16], b: &[u16]) {
        for (x, y) in a.iter().zip(b) {
            let e = x.abs_diff(*y);
            self.n += 1;
            self.sum += e as u64;
            self.max = self.max.max(e);
            self.hist[match e {
                0 => 0,
                1 => 1,
                2 => 2,
                3..=4 => 3,
                5..=16 => 4,
                _ => 5,
            }] += 1;
        }
    }

    fn mean(&self) -> f64 {
        self.sum as f64 / self.n.max(1) as f64
    }

    fn print(&self, name: &str) {
        let p = |k: usize| 100.0 * self.hist[k] as f64 / self.n.max(1) as f64;
        println!(
            "{name}: max {} mean {:.3} | 0: {:.2}% 1: {:.2}% 2: {:.2}% 3-4: {:.2}% 5-16: {:.3}% >16: {:.4}%",
            self.max,
            self.mean(),
            p(0),
            p(1),
            p(2),
            p(3),
            p(4),
            p(5)
        );
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 5 {
        eprintln!("usage: mxl-compare <domain A> <flow A> <domain B> <flow B> [frames] [width height]");
        std::process::exit(2);
    }
    let frames: usize = a.get(5).and_then(|v| v.parse().ok()).unwrap_or(25);
    let w: usize = a.get(6).and_then(|v| v.parse().ok()).unwrap_or(1920);
    let h: usize = a.get(7).and_then(|v| v.parse().ok()).unwrap_or(1080);
    let api = mxlio::load().unwrap_or_else(|e| panic!("{e}"));
    let open = |d: &str, f: &str| {
        let inst = Instance::new(&api, Path::new(d)).unwrap_or_else(|e| panic!("{e}"));
        inst.video_reader(f.parse().expect("flow id")).unwrap_or_else(|e| panic!("{d} {f}: {e}"))
    };
    let (ra, rb) = (open(&a[1], &a[2]), open(&a[3], &a[4]));
    let (mut ya, mut ca, mut yb, mut cb) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let grab = |r: &mxlio::VideoReader, i: u64, y: &mut Vec<u16>, c: &mut Vec<u16>| -> bool {
        match r.grain(i) {
            Ok(Some(g)) => {
                unpack(unsafe { g.data() }, w, h, y, c);
                true
            }
            _ => false,
        }
    };
    // Offset: B's grain i shows A's grain i − d; pick the d with the least
    // luma error on one grain.
    let hb = rb.head().expect("head B").saturating_sub(2);
    assert!(grab(&rb, hb, &mut yb, &mut cb), "grain {hb} of B not readable");
    let mut best = (u64::MAX, f64::MAX);
    for d in 0..8u64 {
        if grab(&ra, hb - d, &mut ya, &mut ca) {
            let mut s = Stats::default();
            s.add(&ya, &yb);
            if s.mean() < best.1 {
                best = (d, s.mean());
            }
        }
    }
    let d = best.0;
    println!("offset: B grain i = A grain i - {d} (mean luma error at that offset {:.3})", best.1);
    let (mut sy, mut sc) = (Stats::default(), Stats::default());
    let mut done = 0;
    let start = rb.head().expect("head B").saturating_sub(frames as u64 + 2);
    for i in start..start + frames as u64 {
        if grab(&rb, i, &mut yb, &mut cb) && grab(&ra, i - d, &mut ya, &mut ca) {
            sy.add(&ya, &yb);
            sc.add(&ca, &cb);
            done += 1;
        }
    }
    println!("grains compared: {done} of {frames} ({w}x{h})");
    sy.print("luma  ");
    sc.print("chroma");
}
