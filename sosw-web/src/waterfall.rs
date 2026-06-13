use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

pub struct Waterfall {
    ctx: CanvasRenderingContext2d,
    width: u32,
    height: u32,
    buffer: Vec<u8>,
    col: u32,
    // Active band overlay
    band_sc_min: Option<usize>,
    band_sc_max: Option<usize>,
    band_fft_size: Option<usize>,
    // Frequency axis labels
    sample_rate: u32,
}

impl Waterfall {
    pub fn new(canvas: HtmlCanvasElement) -> Result<Self, JsValue> {
        let width = canvas.width();
        let height = canvas.height();
        let ctx = canvas
            .get_context("2d")?
            .ok_or("no 2d context")?
            .dyn_into::<CanvasRenderingContext2d>()?;

        let buf_len = (width * height * 4) as usize;
        let buffer = vec![0u8; buf_len];

        Ok(Self {
            ctx,
            width,
            height,
            buffer,
            col: 0,
            band_sc_min: None,
            band_sc_max: None,
            band_fft_size: None,
            sample_rate: 48000,
        })
    }

    pub fn set_active_band(&mut self, sc_min: usize, sc_max: usize, fft_size: usize, sample_rate: u32) {
        self.band_sc_min = Some(sc_min);
        self.band_sc_max = Some(sc_max);
        self.band_fft_size = Some(fft_size);
        self.sample_rate = sample_rate;
    }

    pub fn push_spectrum(&mut self, magnitudes: &[f32]) {
        if magnitudes.is_empty() {
            return;
        }

        let w = self.width as usize;
        let h = self.height as usize;

        // Shift all rows down by 1 row
        let row_bytes = w * 4;
        self.buffer.copy_within(0..(h - 1) * row_bytes, row_bytes);

        // Compute the active band pixel range for overlay
        let (band_x_start, band_x_end) = if let (Some(sc_min), Some(sc_max), Some(fft_size)) =
            (self.band_sc_min, self.band_sc_max, self.band_fft_size)
        {
            let half_bins = fft_size / 2;
            (
                sc_min * w / half_bins,
                (sc_max + 1) * w / half_bins,
            )
        } else {
            (0, 0)
        };

        // Draw new spectrum at y = 0
        for x in 0..w {
            let idx = x * magnitudes.len() / w;
            let mag = magnitudes.get(idx).copied().unwrap_or(0.0);

            let db = 20.0 * mag.log10().max(-6.0).min(0.0);
            let normalized = (db / -6.0).clamp(0.0, 1.0);

            let (r, g, b) = self.hot_colormap(normalized);

            let buf_idx = x * 4;
            if buf_idx + 3 < self.buffer.len() {
                self.buffer[buf_idx] = r;
                self.buffer[buf_idx + 1] = g;
                self.buffer[buf_idx + 2] = b;
                self.buffer[buf_idx + 3] = 255;
            }

            // Draw active band overlay
            if band_x_end > band_x_start {
                if x >= band_x_start && x < band_x_end {
                    let alpha_factor = 0.3;
                    let existing_r = r as f32;
                    let existing_g = g as f32;
                    let existing_b = b as f32;
                    let tint_r = existing_r * (1.0 - alpha_factor) + 80.0 * alpha_factor;
                    let tint_g = existing_g * (1.0 - alpha_factor) + 255.0 * alpha_factor;
                    let tint_b = existing_b * (1.0 - alpha_factor) + 80.0 * alpha_factor;
                    if buf_idx + 3 < self.buffer.len() {
                        self.buffer[buf_idx] = tint_r as u8;
                        self.buffer[buf_idx + 1] = tint_g as u8;
                        self.buffer[buf_idx + 2] = tint_b as u8;
                    }
                }
            }
        }
    }

    pub fn render(&mut self) {
        let image_data = match ImageData::new_with_u8_clamped_array_and_sh(
            wasm_bindgen::Clamped(&self.buffer),
            self.width,
            self.height,
        ) {
            Ok(img) => img,
            Err(_) => return,
        };

        let _ = self.ctx.put_image_data(&image_data, 0.0, 0.0);

        // Draw frequency axis labels
        if let Some(fft_size) = self.band_fft_size {
            let fs = self.sample_rate as f64;
            let w = self.width as f64;
            let h = self.height as f64;
            let _ = self.ctx.set_fill_style_str("#888");
            let _ = self.ctx.set_font("10px monospace");

            // Left label: 0 Hz
            let _ = self.ctx.fill_text("0", 2.0, h - 4.0);

            // Right label: sample_rate/2 Hz
            let right_text = format!("{}k", (fs / 2000.0).round() as u32);
            let _ = self.ctx.fill_text(&right_text, w - 40.0, h - 4.0);

            // Active band labels
            if let (Some(sc_min), Some(sc_max)) = (self.band_sc_min, self.band_sc_max) {
                let bin_w = fs / fft_size as f64;
                let f_min = sc_min as f64 * bin_w;
                let f_max = sc_max as f64 * bin_w;
                let band_label = format!("{:.0}k–{:.0}k", f_min / 1000.0, f_max / 1000.0);
                let label_x = w - 100.0;
                let _ = self.ctx.set_fill_style_str("#4fc3f7");
                let _ = self.ctx.fill_text(&band_label, label_x, 12.0);
            }
        }
    }

    fn hot_colormap(&self, t: f32) -> (u8, u8, u8) {
        let t = t.clamp(0.0, 1.0);
        if t < 0.25 {
            let v = (t / 0.25 * 255.0) as u8;
            (0, 0, v)
        } else if t < 0.5 {
            let v = ((t - 0.25) / 0.25 * 255.0) as u8;
            (0, v, 255)
        } else if t < 0.75 {
            let v = ((t - 0.5) / 0.25 * 255.0) as u8;
            (v, 255, 255 - v)
        } else {
            let v = ((t - 0.75) / 0.25 * 255.0) as u8;
            (255, 255 - v, 0)
        }
    }
}

pub fn compute_spectrum(samples: &[f32], fft_size: usize) -> Vec<f32> {
    use rustfft::FftPlanner;
    use num_complex::Complex32;

    if samples.len() < fft_size {
        return vec![0.0f32; fft_size / 2];
    }

    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(fft_size);

    let step = fft_size / 2;
    let n_windows = (samples.len() - fft_size) / step + 1;
    let mut spectrum = vec![0.0f32; fft_size / 2];

    for win_idx in 0..n_windows.min(4) {
        let start = win_idx * step;
        let mut buf: Vec<Complex32> = (0..fft_size)
            .map(|i| {
                let hann =
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (fft_size - 1) as f32).cos());
                Complex32::new(samples[start + i] * hann, 0.0)
            })
            .collect();

        fft.process(buf.as_mut_slice());

        for i in 0..fft_size / 2 {
            spectrum[i] += buf[i].norm() / n_windows as f32;
        }
    }

    spectrum
}
