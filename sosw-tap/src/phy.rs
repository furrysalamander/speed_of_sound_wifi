use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::preamble::{compute_cross_correlation, find_preamble_peak, generate_preamble_audio};
use sosw_core::link::frame::{FrameAssembler, FrameParser, ParsedFrame};
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use sosw_core::Config;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEFAULT_TX_GAIN: f32 = 12.0;

const CARRIER_SENSE_THRESHOLD: f32 = 0.03;

struct AudioBuffer {
    samples: VecDeque<f32>,
}

pub struct Phy {
    config: Config,
    modulator: OfdmModulator,
    demodulator: OfdmDemodulator,
    frame_assembler: FrameAssembler,
    frame_parser: FrameParser,
    preamble_audio: Vec<f32>,
    preamble_energy: f32,
    rx_batch: Vec<f32>,
    rx_skip: usize,
    rx_samples: Arc<Mutex<AudioBuffer>>,
    tx_samples: Arc<Mutex<AudioBuffer>>,
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
    rx_sample_count: Arc<AtomicUsize>,
    rx_mute_at: usize,
    muted: bool,
}

impl Phy {
    pub fn new(
        config: Config,
        tx_device: Option<&str>,
        rx_device: Option<&str>,
    ) -> Result<Self> {
        let host = cpal::default_host();

        let out_device = device_by_name(&host, tx_device, true)?;
        let in_device = device_by_name(&host, rx_device, false)?;

        let mut out_config = out_device.default_output_config()?.config();
        let mut in_config = in_device.default_input_config()?.config();
        // Request small buffers to minimize PipeWire latency.
        out_config.buffer_size = cpal::BufferSize::Fixed(256);
        in_config.buffer_size = cpal::BufferSize::Fixed(256);

        if in_config.sample_rate != config.sample_rate {
            anyhow::bail!(
                "Sample rate mismatch: cpal={}, config={}",
                in_config.sample_rate,
                config.sample_rate
            );
        }

        let rx_samples: Arc<Mutex<AudioBuffer>> =
            Arc::new(Mutex::new(AudioBuffer { samples: VecDeque::new() }));
        let tx_samples: Arc<Mutex<AudioBuffer>> =
            Arc::new(Mutex::new(AudioBuffer { samples: VecDeque::new() }));
        let tx_running = Arc::new(AtomicBool::new(false));
        let rx_sample_count: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));

        let rx_handle = rx_samples.clone();
        let rx_count = rx_sample_count.clone();
        let in_channels = in_config.channels;
        let input_stream = in_device.build_input_stream(
            in_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let n = data.len();
                rx_count.fetch_add(n, Ordering::Relaxed);
                let mut buf = rx_handle.lock().unwrap();
                if in_channels >= 2 {
                    let frames = n / in_channels as usize;
                    for i in 0..frames {
                        // Take left channel only
                        buf.samples.push_back(data[i * in_channels as usize]);
                    }
                } else {
                    buf.samples.extend(data.iter().copied());
                }
            },
            |err| eprintln!("input stream error: {}", err),
            None,
        )?;

        let tx_handle = tx_samples.clone();
        let tx_flag = tx_running.clone();
        let out_channels = out_config.channels;
        let output_stream = out_device.build_output_stream(
            out_config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut buf = tx_handle.lock().unwrap();
                tx_flag.store(true, Ordering::Relaxed);
                if out_channels >= 2 {
                    let frames = data.len() / out_channels as usize;
                    for i in 0..frames {
                        let s = buf.samples.pop_front().unwrap_or(0.0);
                        for ch in 0..out_channels as usize {
                            data[i * out_channels as usize + ch] = s;
                        }
                    }
                } else {
                    for sample in data.iter_mut() {
                        *sample = buf.samples.pop_front().unwrap_or(0.0);
                    }
                }
                if buf.samples.is_empty() {
                    tx_flag.store(false, Ordering::Relaxed);
                }
            },
            |err| eprintln!("output stream error: {}", err),
            None,
        )?;

        input_stream.play()?;
        output_stream.play()?;

        let preamble_audio = generate_preamble_audio(&config);
        let preamble_energy: f32 =
            preamble_audio.iter().map(|&s| s * s).sum::<f32>().max(1e-12);

        let modulator = OfdmModulator::new(&config);
        let demodulator = OfdmDemodulator::new(&config);
        let frame_assembler = FrameAssembler::new(&config);
        let frame_parser = FrameParser::new(&config);

        Ok(Self {
            config,
            modulator,
            demodulator,
            frame_assembler,
            frame_parser,
            preamble_audio,
            preamble_energy,
            rx_batch: Vec::new(),
            rx_skip: 0,
            rx_samples,
            tx_samples,
            _input_stream: input_stream,
            _output_stream: output_stream,
            rx_sample_count,
            rx_mute_at: 0,
            muted: false,
                })
    }

    pub fn carrier_sense(&mut self) -> bool {
        let buf = {
            let rx = self.rx_samples.lock().unwrap();
            let len = rx.samples.len();
            if len < self.config.preamble_samples() {
                return false;
            }
            let drain = (len / self.config.symbol_duration_samples())
                * self.config.symbol_duration_samples();
            let start = drain.saturating_sub(self.config.preamble_samples());
            rx.samples.iter().skip(start).copied().collect::<Vec<f32>>()
        };

        if buf.len() < self.config.preamble_samples() {
            return false;
        }

        let corr = compute_cross_correlation(&buf, &self.preamble_audio);
        if let Some((_, norm)) = find_preamble_peak(&corr, self.preamble_energy, &buf, self.preamble_audio.len()) {
            return norm > CARRIER_SENSE_THRESHOLD;
        }
        false
    }

    pub fn receive_frame(&mut self) -> Option<ParsedFrame> {
        {
            let mut rx = self.rx_samples.lock().unwrap();
            self.rx_batch.extend(rx.samples.drain(..));
        }

        // Advance past any known false-positive region.
        if self.rx_skip > 0 {
            let to_drain = self.rx_skip.min(self.rx_batch.len());
            self.rx_batch.drain(..to_drain);
            self.rx_skip = 0;
        }

        if self.rx_batch.len() < self.config.frame_samples() {
            return None;
        }

        if self.rx_batch.len() > self.config.frame_samples() * 200 {
            let excess = self.rx_batch.len() - self.config.frame_samples() * 200;
            self.rx_batch.drain(..excess);
        }

        let demod_result = self.demodulator.process_samples(&self.rx_batch);
        if let Some(ref r) = demod_result {
            eprintln!("  [phy] frame: peak={:.3} cfo={:.4} |H|={:.4} consumed={} batch={}",
                r.preamble_peak, r.cfo_rad_per_sym, r.mean_h_magnitude, r.consumed_samples, self.rx_batch.len());
        }

        let result = match demod_result {
            Some(r) => r,
            None => {
                self.rx_skip = self.config.preamble_samples() / 4;
                self.demodulator.reset();
                return None;
            }
        };
        let consumed = result.consumed_samples.min(self.rx_batch.len());
        self.frame_parser = FrameParser::new(&self.config);
        if result.preamble_peak > 0.5 {
            eprintln!("    [phy] first 16 bytes: {:02x?}", &result.bytes[..16.min(result.bytes.len())]);
        }
        let mut frames = self.frame_parser.feed_bytes(&result.bytes);
        if frames.is_empty() {
            self.rx_batch.drain(..consumed);
            self.demodulator.reset();
            return None;
        }
        let frame = frames.remove(0);
        if !frame.valid {
            self.rx_batch.drain(..consumed);
            return None;
        }
        self.rx_batch.drain(..consumed);
        Some(frame)
    }

    pub fn flush_rx(&mut self) {
        let mut rx = self.rx_samples.lock().unwrap();
        rx.samples.clear();
        drop(rx);
        self.rx_batch.clear();
        self.demodulator.reset();
        self.frame_parser = FrameParser::new(&self.config);
    }

    pub fn transmit_frame(&mut self, data: &[u8], frame_type: u8) -> Vec<f32> {
        let frame = self.frame_assembler.assemble_frame_with_type(data, frame_type);
        let mut audio = self.modulator.modulate_with_preamble(&frame);
        // Guard interval: sym_dur silence prevents false preamble detection
        // from the previous frame's tail data during cross-correlation.
        audio.extend(std::iter::repeat(0.0f32).take(self.config.symbol_duration_samples()));
        audio
    }

    pub fn play_samples(&self, samples: &[f32]) {
        let mut tx = self.tx_samples.lock().unwrap();
        for &s in samples {
            tx.samples.push_back(s * DEFAULT_TX_GAIN);
        }
    }

    pub fn wait_tx_done(&self, sample_count: usize) {
        let duration =
            Duration::from_secs_f64(sample_count as f64 / self.config.sample_rate as f64);
        let margin = Duration::from_millis(50);
        std::thread::sleep(duration + margin);
    }

    pub fn begin_tx_mute(&mut self) {
        self.rx_mute_at = self.rx_sample_count.load(Ordering::Relaxed);
        self.muted = true;
    }

    pub fn end_tx_mute(&mut self) {
        let now = self.rx_sample_count.load(Ordering::Relaxed);
        let captured_during_tx = now.saturating_sub(self.rx_mute_at);
        {
            let mut rx = self.rx_samples.lock().unwrap();
            let to_drain = captured_during_tx.min(rx.samples.len());
            rx.samples.drain(..to_drain);
        }
        self.muted = false;
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn stats(&mut self) -> PhyStats {
        let rx_buf = self.rx_samples.lock().unwrap();
        let tx_buf = self.tx_samples.lock().unwrap();
        PhyStats {
            rx_buffered: rx_buf.samples.len(),
            tx_buffered: tx_buf.samples.len(),
            rx_batch_pending: self.rx_batch.len(),
        }
    }
}

#[derive(Default)]
pub struct PhyStats {
    pub rx_buffered: usize,
    pub tx_buffered: usize,
    pub rx_batch_pending: usize,
}

fn device_by_name(
    host: &cpal::Host,
    name: Option<&str>,
    is_output: bool,
) -> Result<cpal::Device> {
    match name {
        Some(n) => {
            let lower = n.to_lowercase();
            let dev = if is_output {
                host.output_devices()
            } else {
                host.input_devices()
            }?
            .find(|d| {
                d.id()
                    .map(|id| format!("{}", id).to_lowercase().contains(&lower))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("device matching '{}' not found", n))?;
            Ok(dev)
        }
        None => {
            if is_output {
                host.default_output_device()
                    .ok_or_else(|| anyhow::anyhow!("no default output device"))
            } else {
                host.default_input_device()
                    .ok_or_else(|| anyhow::anyhow!("no default input device"))
            }
        }
    }
}
