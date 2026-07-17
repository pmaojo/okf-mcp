//! Micro-benchmark std-only: sin criterion (regla del workspace),
//! con `std::time::Instant` a pelo — el mismo espíritu de "escribirlo
//! a mano para entender qué mide una herramienta como criterion".
//!
//! Uso: cargo run --release --example bench -p hash-core

use hash_core::sha256;
use std::time::Instant;

fn main() {
    let sizes_mb = [1usize, 8, 64];
    let repeats = 5;

    for &mb in &sizes_mb {
        let data = vec![0x5au8; mb * 1024 * 1024];
        let mut best = f64::MAX;

        for _ in 0..repeats {
            let start = Instant::now();
            let digest = sha256(&data);
            let elapsed = start.elapsed().as_secs_f64();
            std::hint::black_box(digest);
            best = best.min(elapsed);
        }

        let throughput = (mb as f64) / best;
        println!("{mb:>4} MiB: mejor de {repeats} = {best:.4}s -> {throughput:.1} MiB/s");
    }
}
