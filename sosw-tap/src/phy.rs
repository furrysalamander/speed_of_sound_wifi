use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::physical::preamble::{compute_cross_correlation, generate_preamble_audio};
use sosw_core::link::frame::{FrameAssembler, FrameParser, ParsedFrame};
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use sosw_core::Config;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    rx_samples: Arc<Mutex<AudioBuffer>>,
    tx_samples: Arc<Mutex<AudioBuffer>>,
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
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

        let out_config = out_device.default_output_config()?.config();
        let in_config = in_device.default_input_config()?.config();

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

        let rx_handle = rx_samples.clone();
        let input_stream = in_device.build_input_stream(
            in_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let mut buf = rx_handle.lock().unwrap();
                buf.samples.extend(data.iter().copied());
            },
            |err| eprintln!("input stream error: {}", err),
            None,
        )?;

        let tx_handle = tx_samples.clone();
        let tx_flag = tx_running.clone();
        let output_stream = out_device.build_output_stream(
            out_config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut buf = tx_handle.lock().unwrap();
                tx_flag.store(true, Ordering::Relaxed);
                for sample in data.iter_mut() {
                    *sample = buf.samples.pop_front().unwrap_or(0.0);
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
            rx_samples,
            tx_samples,
            _input_stream: input_stream,
            _output_stream: output_stream,
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
        let peak = corr.iter().cloned().fold(0.0f32, f32::max);
        let signal_energy: f32 = buf.iter().map(|&s| s * s).sum::<f32>().max(1e-12);
        let norm = peak / (self.preamble_energy * signal_energy).sqrt().max(1e-12);
        norm > CARRIER_SENSE_THRESHOLD
    }

    pub fn receive_frame(&mut self) -> Option<ParsedFrame> {
        {
            let mut rx = self.rx_samples.lock().unwrap();
            self.rx_batch.extend(rx.samples.drain(..));
        }

        if self.rx_batch.len() < self.config.frame_samples() {
            return None;
        }

        if self.rx_batch.len() > self.config.frame_samples() * 4 {
            let excess = self.rx_batch.len() - self.config.frame_samples() * 4;
            self.rx_batch.drain(..excess);
        }

        let result = self.demodulator.process_samples(&self.rx_batch)?;
        let consumed = result.consumed_samples.min(self.rx_batch.len());
        self.rx_batch.drain(..consumed);

        let mut frames = self.frame_parser.feed_bytes(&result.bytes);
        if frames.is_empty() {
            return None;
        }
        let frame = frames.remove(0);
        if !frame.valid {
            return None;
        }
        Some(frame)
    }

    pub fn flush_rx(&mut self) {
        let mut rx = self.rx_samples.lock().unwrap();
        rx.samples.clear();
        drop(rx);
        self.rx_batch.clear();
        self.demodulator.reset();
    }

    pub fn transmit_frame(&mut self, data: &[u8], frame_type: u8) -> Vec<f32> {
        let frame = self.frame_assembler.assemble_frame_with_type(data, frame_type);
        self.modulator.modulate_with_preamble(&frame)
    }

    pub fn play_samples(&self, samples: &[f32]) {
        let mut tx = self.tx_samples.lock().unwrap();
        tx.samples.extend(samples.iter().copied());
    }

    pub fn wait_tx_done(&self, sample_count: usize) {
        let duration =
            Duration::from_secs_f64(sample_count as f64 / self.config.sample_rate as f64);
        let margin = Duration::from_millis(50);
        std::thread::sleep(duration + margin);
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
