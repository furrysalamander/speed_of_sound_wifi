use sosw_core::config::Config;
use sosw_core::physical::preamble;

#[test]
fn check_preamble() {
    let config = Config::ofdm_default();
    let symbols = preamble::generate_preamble_symbols(&config);
    
    println!("Preamble symbols generated: {} x {}", symbols.len(), symbols[0].len());
    for sym_idx in 0..2.min(symbols.len()) {
        for sc in 0..10.min(symbols[sym_idx].len()) {
            let s = symbols[sym_idx][sc];
            let b0 = if s.re >= 0.0 { 0 } else { 1 };
            let b1 = if s.im >= 0.0 { 0 } else { 1 };
            println!("  sym[{}][{}] = ({}, {})", sym_idx, sc, b0, b1);
        }
    }

    // Check preamble audio autocorrelation
    let audio = preamble::generate_preamble_audio(&config);
    let n = audio.len();
    println!("\nPreamble audio length: {}", n);
    
    let energy: f32 = audio.iter().map(|&s| s * s).sum();
    println!("  energy: {:.4}", energy);

    for &offset in &[0usize, 32, 288, 576] {
        let len = n - offset;
        if len > 0 {
            let corr: f32 = audio[..len].iter().zip(audio[offset..].iter())
                .map(|(a, b)| a * b).sum();
            println!("  autocorr at offset {}: {:.4}", offset, corr);
        }
    }
}
