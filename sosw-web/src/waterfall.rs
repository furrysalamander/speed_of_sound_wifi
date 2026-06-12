use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

pub struct Waterfall {
    ctx: CanvasRenderingContext2d,
    width: u32,
    height: u32,
    buffer: Vec<u8>,
    col: u32,
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
        })
    }

    pub fn push_spectrum(&mut self, magnitudes: &[f32]) {
        if magnitudes.is_empty() {
            return;
        }

        let w = self.width as usize;
        let h = self.height as usize;

        if self.col >= w as u32 {
            self.col = 0;
        }

        for y in 0..h {
            let idx = y * magnitudes.len() / h;
            let mag = magnitudes.get(idx).copied().unwrap_or(0.0);

            let db = 20.0 * mag.log10().max(-6.0).min(0.0);
            let normalized = (db / -6.0).clamp(0.0, 1.0);

            let (r, g, b) = self.hot_colormap(normalized);

            let buf_idx = (y * w + self.col as usize) * 4;
            if buf_idx + 3 < self.buffer.len() {
                self.buffer[buf_idx] = r;
                self.buffer[buf_idx + 1] = g;
                self.buffer[buf_idx + 2] = b;
                self.buffer[buf_idx + 3] = 255;
            }
        }

        self.col += 1;
    }

    pub fn render(&mut self) {
        let _w = self.width as usize;
        let _h = self.height as usize;

        let image_data = match ImageData::new_with_u8_clamped_array_and_sh(
            wasm_bindgen::Clamped(&self.buffer),
            self.width,
            self.height,
        ) {
            Ok(img) => img,
            Err(_) => return,
        };

        let _ = self.ctx.put_image_data(&image_data, 0.0, 0.0);
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
