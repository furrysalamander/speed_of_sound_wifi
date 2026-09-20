//! Reusable full-duplex audio endpoint for link and measurement tools.
//!
//! Modeled on the proven `sosw_link` audio path: output samples are queued in a
//! shared buffer that the output callback drains, input samples accumulate in a
//! shared buffer. Playback can optionally mute the local receive path so a
//! node does not decode its own speaker echo.

use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct DuplexAudio {
    tx: Arc<Mutex<Vec<f32>>>,
    tx_pos: Arc<AtomicUsize>,
    rx: Arc<Mutex<Vec<f32>>>,
    muting: Arc<AtomicBool>,
    pub sample_rate: u32,
    pub in_channels: usize,
    pub out_channels: usize,
    _out: cpal::Stream,
    _in: cpal::Stream,
}

/// Find an audio device whose id contains `name` (case-insensitive).
pub fn pick_device(host: &cpal::Host, name: Option<&str>, output: bool) -> Result<cpal::Device> {
    match name {
        Some(n) => {
            let lower = n.to_lowercase();
            let dev = if output {
                host.output_devices()?
            } else {
                host.input_devices()?
            }
            .find(|d| {
                d.id()
                    .map(|id| format!("{}", id).to_lowercase().contains(&lower))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("no {} device matching '{}'", if output { "output" } else { "input" }, n))?;
            Ok(dev)
        }
        None => {
            if output {
                host.default_output_device()
                    .ok_or_else(|| anyhow::anyhow!("no default output device"))
            } else {
                host.default_input_device()
                    .ok_or_else(|| anyhow::anyhow!("no default input device"))
            }
        }
    }
}

impl DuplexAudio {
    pub fn new(tx_dev: Option<&str>, rx_dev: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let out_dev = pick_device(&host, tx_dev, true)?;
        let in_dev = pick_device(&host, rx_dev, false)?;
        let mut out_cfg = out_dev.default_output_config()?.config();
        let mut in_cfg = in_dev.default_input_config()?.config();
        // Request small callback periods. With cpal's PulseAudio backend a
        // Fixed buffer size sets the end-to-end latency target; the Default
        // lets the server choose a very large buffer (multi-second here).
        out_cfg.buffer_size = cpal::BufferSize::Fixed(256);
        in_cfg.buffer_size = cpal::BufferSize::Fixed(256);
        let in_channels = in_cfg.channels as usize;
        let out_channels = out_cfg.channels as usize;
        let sample_rate = in_cfg.sample_rate;

        eprintln!(
            "TX {} ({} Hz, {} ch) | RX {} ({} Hz, {} ch)",
            out_dev.id().map(|i| format!("{}", i)).unwrap_or_default(),
            out_cfg.sample_rate,
            out_channels,
            in_dev.id().map(|i| format!("{}", i)).unwrap_or_default(),
            in_cfg.sample_rate,
            in_channels,
        );

        let tx = Arc::new(Mutex::new(Vec::<f32>::new()));
        let rx = Arc::new(Mutex::new(Vec::<f32>::new()));
        let tx_pos = Arc::new(AtomicUsize::new(0));
        let muting = Arc::new(AtomicBool::new(false));

        let t = tx.clone();
        let p = tx_pos.clone();
        let out = out_dev.build_output_stream::<f32, _, _>(
            out_cfg,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let buf = t.lock().unwrap();
                let base = p.load(Ordering::Relaxed);
                let frames = data.len() / out_channels;
                for i in 0..frames {
                    let s = buf.get(base + i).copied().unwrap_or(0.0);
                    for c in 0..out_channels {
                        data[i * out_channels + c] = s;
                    }
                }
                p.fetch_add(frames, Ordering::Relaxed);
            },
            |e| eprintln!("out error: {}", e),
            None,
        )?;

        let r = rx.clone();
        let m = muting.clone();
        let in_stream = in_dev.build_input_stream::<f32, _, _>(
            in_cfg,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                if m.load(Ordering::Relaxed) {
                    return;
                }
                if let Ok(mut v) = r.lock() {
                    if in_channels > 1 {
                        for f in data.chunks(in_channels) {
                            v.push(f[0]);
                        }
                    } else {
                        v.extend_from_slice(data);
                    }
                }
            },
            |e| eprintln!("in error: {}", e),
            None,
        )?;
        out.play()?;
        in_stream.play()?;
        Ok(Self {
            tx,
            tx_pos,
            rx,
            muting,
            sample_rate,
            in_channels,
            out_channels,
            _out: out,
            _in: in_stream,
        })
    }

    /// Queue samples for playback immediately (non-blocking).
    pub fn play_now(&self, samples: &[f32]) {
        let mut t = self.tx.lock().unwrap();
        t.clear();
        t.extend_from_slice(samples);
        self.tx_pos.store(0, Ordering::Relaxed);
    }

    /// Queue samples and wait for playback plus `tail` to finish.
    pub fn play_blocking(&self, samples: &[f32], tail: Duration) {
        self.play_now(samples);
        let dur = Duration::from_secs_f64(samples.len() as f64 / self.sample_rate as f64);
        std::thread::sleep(dur + tail);
    }

    pub fn is_playing(&self) -> bool {
        let t = self.tx.lock().unwrap();
        self.tx_pos.load(Ordering::Relaxed) < t.len()
    }

    /// Play with the local receive path muted, then keep it muted for `tail`
    /// after the audio ends (to ride out the room echo).
    pub fn play_muted(&self, samples: &[f32], tail: Duration) {
        self.muting.store(true, Ordering::Relaxed);
        self.play_blocking(samples, tail);
        self.muting.store(false, Ordering::Relaxed);
    }

    pub fn clear_rx(&self) {
        self.rx.lock().unwrap().clear();
    }

    pub fn rx_len(&self) -> usize {
        self.rx.lock().unwrap().len()
    }

    /// Drain and return all recorded samples so far.
    pub fn take_rx(&self) -> Vec<f32> {
        let mut r = self.rx.lock().unwrap();
        r.drain(..).collect()
    }

    /// Wait until at least `n` samples are buffered, or `timeout` elapses.
    pub fn wait_rx(&self, n: usize, timeout: Duration) -> bool {
        let start = Instant::now();
        while self.rx_len() < n {
            if start.elapsed() > timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Record for a fixed duration and return the captured samples.
    pub fn record_for(&self, dur: Duration) -> Vec<f32> {
        self.clear_rx();
        std::thread::sleep(dur);
        self.take_rx()
    }

    /// Peak absolute sample of the currently buffered input (for level checks).
    pub fn rx_peak(&self) -> f32 {
        self.rx.lock().unwrap().iter().map(|s| s.abs()).fold(0.0, f32::max)
    }
}

impl Drop for DuplexAudio {
    fn drop(&mut self) {
        // Streams are stopped when their handles drop.
    }
}

// ---------------------------------------------------------------------------
// Capture file I/O: raw little-endian f32, so captures can be moved between
// machines and re-analyzed offline (a technique this project relies on).
// ---------------------------------------------------------------------------

pub fn save_f32(path: &str, samples: &[f32]) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, bytes)
}

pub fn load_f32(path: &str) -> std::io::Result<Vec<f32>> {
    let bytes = std::fs::read(path)?;
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

/// Write a mono 16-bit PCM WAV, for listening to captures in a player.
pub fn save_wav(path: &str, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, out)
}

/// Load a mono WAV (16-bit PCM) or raw f32 capture based on the extension.
pub fn load_capture(path: &str) -> std::io::Result<Vec<f32>> {
    if path.ends_with(".wav") {
        let bytes = std::fs::read(path)?;
        // Minimal RIFF parse: find the `data` chunk.
        let mut pos = 12usize;
        while pos + 8 <= bytes.len() {
            let id = &bytes[pos..pos + 4];
            let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
            if id == b"data" {
                let start = pos + 8;
                let end = (start + len).min(bytes.len());
                return Ok(bytes[start..end]
                    .chunks_exact(2)
                    .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
                    .collect());
            }
            pos += 8 + len + (len & 1);
        }
        Ok(Vec::new())
    } else {
        load_f32(path)
    }
}
