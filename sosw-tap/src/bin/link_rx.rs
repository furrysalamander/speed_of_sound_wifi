use clap::Parser;
use sosw_core::physical::dtmf::{self, DtmfConfig};
use sosw_tap::link;
#[derive(Parser)]
struct Args {
    file: String,
    #[arg(long, default_value_t = 100)]
    symbol_ms: u32,
    #[arg(long, default_value_t = 100)]
    gap_ms: u32,
}
fn main() {
    let a = Args::parse();
    let cfg = DtmfConfig {
        sample_rate: 48000,
        symbol_samples: 48 * a.symbol_ms as usize,
        gap_samples: 48 * a.gap_ms as usize,
        amplitude: 0.4,
    };
    let b = std::fs::read(&a.file).unwrap();
    let x: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect();
    let syms = dtmf::decode(&x, &cfg);
    eprintln!("{} symbols: {:X?}", syms.len(), syms);
    for (m, _) in link::decode_all(&syms) {
        eprintln!("DECODED kind={} node={} param={}", m.kind, m.node_id, m.param);
    }
}
