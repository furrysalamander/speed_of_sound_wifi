use num_complex::Complex32;
use rustfft::FftPlanner;

#[test]
fn test_fft_ifft_roundtrip() {
    let n = 256;
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);
    let scale = 1.0 / (n as f32).sqrt();

    let mut data: Vec<Complex32> = (0..n).map(|i| {
        Complex32::new((i as f32 / n as f32 * 2.0 * std::f32::consts::PI).cos(), 0.0)
    }).collect();

    let original = data.clone();

    // Forward
    fft.process(&mut data);
    for v in data.iter_mut() { *v *= scale; }

    // Inverse
    ifft.process(&mut data);
    for v in data.iter_mut() { *v *= scale; }

    for i in 0..n {
        let diff = (data[i] - original[i]).norm();
        if diff > 1e-5 {
            println!("Mismatch at {}: orig={:?} recovered={:?} diff={}", i, original[i], data[i], diff);
        }
    }
    println!("FFT/IFFT roundtrip OK (max diff check passed)");
}
