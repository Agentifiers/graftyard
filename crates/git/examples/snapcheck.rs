//! Streams a repository tier snapshot from stdin through `graftyard_fold::FrameCheck`, without holding it, and prints
//! the trailer as JSON: what BF-20 checks a hosted snapshot with (`tests/m2/results/m2-wp1.md`).
//!
//!   wrangler r2 object get <bucket>/<key> --remote --pipe | cargo run --release -p graftyard-git --example snapcheck -- core|metadata
//!
//! Exit 0 with the trailer when the frames' row count and digest match it; exit 1 with the error otherwise.

use std::io::Read;

use graftyard_fold::{FrameCheck, decode_some, frames_header};

fn main() {
    let schema = match std::env::args().nth(1).as_deref() {
        Some("core") => graftyard_git::tiers::CORE,
        Some("metadata") => graftyard_git::tiers::METADATA,
        _ => {
            eprintln!("usage: snapcheck core|metadata < snapshot");
            std::process::exit(2);
        }
    };
    let mut input = std::io::stdin().lock();
    let mut head = vec![0u8; frames_header().len()];
    if input.read_exact(&mut head).is_err() || head != frames_header() {
        eprintln!("not a frame stream of this format");
        std::process::exit(1);
    }
    let mut fc = FrameCheck::new(schema);
    let (mut buf, mut chunk, mut bytes, mut frames) = (Vec::new(), vec![0u8; 1 << 20], head.len() as u64, 0u64);
    loop {
        let n = input.read(&mut chunk).expect("stdin");
        if n == 0 {
            break;
        }
        bytes += n as u64;
        buf.extend_from_slice(&chunk[..n]);
        let (fs, used) = decode_some(&buf).unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1)
        });
        for f in &fs {
            if let Err(e) = fc.frame(f) {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        frames += fs.len() as u64;
        buf.drain(..used);
    }
    if !buf.is_empty() {
        eprintln!("{} bytes after the last whole frame", buf.len());
        std::process::exit(1);
    }
    match fc.finish() {
        Ok(tr) => println!(
            "{{\"ok\":true,\"tier\":\"{:?}\",\"to\":{},\"rows\":{},\"digest\":\"{}\",\"frames\":{frames},\"bytes\":{bytes}}}",
            tr.tier,
            tr.to.0,
            tr.rows,
            tr.fold_digest.hex()
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}
