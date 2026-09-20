use sosw_core::physical::dtmf::{self, DtmfConfig};
use sosw_tap::link::{self, Message};
use clap::Parser;
#[derive(Parser)]
struct Args { kind: u8, #[arg(long,default_value_t=2)] node_id: u8, #[arg(long,default_value_t=8)] param: u8, #[arg(long)] out: Option<String> }
fn main() {
    let a = Args::parse();
    let cfg = DtmfConfig::default();
    let msg = match a.kind { 0=>Message::hello(a.node_id,a.param,""),1=>Message::hello_ack(a.node_id,a.param,""),2=>Message::train(a.node_id,a.param),3=>Message::train_ack(a.node_id,a.param),_=>unreachable!() };
    let audio = link::encode_message(&msg, &cfg);
    let n = cfg.symbol_samples;
    let period = cfg.symbol_samples + cfg.gap_samples;
    eprintln!("logical: {:X?}", link::encode_symbols(&msg));
    for (idx, s) in link::encode_symbols(&msg).iter().enumerate() {
        let start = idx*period;
        let end = (start+n).min(audio.len());
        if start>=audio.len() {break;}
        let seg=&audio[start..end];
        let rms=(seg.iter().map(|v|v*v).sum::<f32>()/seg.len() as f32).sqrt();
        eprintln!("sym {:X}: rms={:.4}", s, rms);
    }
    if let Some(p)=a.out { let mut b=Vec::new(); for s in &audio { b.extend_from_slice(&s.to_le_bytes()); } std::fs::write(p,b).unwrap(); }
    let max=audio.iter().map(|s|s.abs()).fold(0.0f32,f32::max);
    eprintln!("total max={:.4}", max);
}
