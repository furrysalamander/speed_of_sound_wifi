use num_complex::Complex32;

pub fn qpsk_map(bits: &[u8]) -> Vec<Complex32> {
    debug_assert!(bits.len() % 2 == 0);
    bits.chunks(2)
        .map(|b| {
            match (b[0], b[1]) {
                (0, 0) => Complex32::new(1.0, 1.0),
                (1, 0) => Complex32::new(-1.0, 1.0),
                (1, 1) => Complex32::new(-1.0, -1.0),
                (0, 1) => Complex32::new(1.0, -1.0),
                _ => unreachable!(),
            }
            .unscale(std::f32::consts::SQRT_2)
        })
        .collect()
}

pub fn qpsk_demap(symbols: &[Complex32]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(symbols.len() * 2);
    for s in symbols {
        bits.push(if s.re >= 0.0 { 0 } else { 1 });
        bits.push(if s.im >= 0.0 { 0 } else { 1 });
    }
    bits
}

pub fn qpsk_hard_decision(symbols: &[Complex32]) -> Vec<Complex32> {
    symbols
        .iter()
        .map(|s| {
            let re = if s.re >= 0.0 { 1.0_f32 } else { -1.0 };
            let im = if s.im >= 0.0 { 1.0_f32 } else { -1.0 };
            Complex32::new(re, im).unscale(std::f32::consts::SQRT_2)
        })
        .collect()
}
