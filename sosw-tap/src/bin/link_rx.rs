use clap::Parser;
use sosw_core::physical::dtmf::{self, DtmfConfig};
use sosw_tap::link;
#[derive(Parser)]
struct Args { file: String }
fn main() {
    let a = Args::parse();
    let cfg = DtmfConfig::default();
    let b = std::fs::read(&a.file).unwrap();
    let x: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect();
    let syms = dtmf::decode(&x, &cfg);
    eprintln!("{} symbols: {:X?}", syms.len(), syms);
    for (m, _) in link::decode_all(&syms) {
        eprintln!("DECODED kind={} node={} param={}", m.kind, m.node_id, m.param);
    }
}
