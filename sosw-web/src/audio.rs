#![allow(dead_code)]
use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::DemodResult;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// ── RX ──────────────────────────────────────────────────────────

pub struct RxHandle {
    demodulator: OfdmDemodulator,
    shared_buffer: Box<std::cell::RefCell<Vec<f32>>>,
    local_buffer: Vec<f32>,
    preamble_samples: usize,
    batch_cap: usize,
    audio_ctx: web_sys::AudioContext,
    _processor: web_sys::ScriptProcessorNode,
    _source: web_sys::MediaStreamAudioSourceNode,
}

impl Drop for RxHandle {
    fn drop(&mut self) {
        _ = self.audio_ctx.close();
    }
}

impl RxHandle {
    pub fn poll(&mut self) -> (Vec<f32>, Option<DemodResult>) {
        let new_samples: Vec<f32> = self.shared_buffer.borrow_mut().drain(..).collect();
        self.local_buffer.extend_from_slice(&new_samples);

        if self.local_buffer.len() > self.batch_cap {
            let excess = self.local_buffer.len() - self.batch_cap;
            self.local_buffer.drain(..excess);
        }

        let mut result = None;
        if self.local_buffer.len() >= self.preamble_samples {
            if let Some(res) = self.demodulator.process_samples(&self.local_buffer) {
                let consumed = res.consumed_samples;
                self.local_buffer.drain(..consumed.min(self.local_buffer.len()));
                result = Some(res);
            } else {
                let drain = (self.preamble_samples / 4).min(self.local_buffer.len());
                self.local_buffer.drain(..drain);
                self.demodulator.reset();
            }
        }

        (new_samples, result)
    }

    pub fn stop(self) {
        drop(self);
    }
}

pub async fn start_rx(config: &Config) -> Result<RxHandle, JsValue> {
    let opts = web_sys::AudioContextOptions::new();
    opts.set_sample_rate(config.sample_rate as f32);
    let ctx = web_sys::AudioContext::new_with_context_options(&opts)?;
    let _ = ctx.resume();

    let window = web_sys::window().ok_or(JsValue::from_str("no window"))?;
    let navigator = window.navigator();
    let media_devices = navigator.media_devices()?;
    let md_ref: &wasm_bindgen::JsValue = media_devices.as_ref();
    if md_ref.is_undefined() || md_ref.is_null() {
        return Err(JsValue::from_str(
            "microphone access requires HTTPS or localhost",
        ));
    }

    let mut audio_constraints = web_sys::MediaTrackConstraints::new();
    audio_constraints.echo_cancellation(&wasm_bindgen::JsValue::FALSE);
    audio_constraints.noise_suppression(&wasm_bindgen::JsValue::FALSE);
    audio_constraints.auto_gain_control(&wasm_bindgen::JsValue::FALSE);

    let mut constraints = web_sys::MediaStreamConstraints::new();
    constraints.audio(&audio_constraints);

    let promise = media_devices.get_user_media_with_constraints(&constraints)?;
    let stream = wasm_bindgen_futures::JsFuture::from(promise)
        .await?
        .dyn_into::<web_sys::MediaStream>()?;

    let source = ctx.create_media_stream_source(&stream)?;

    // ScriptProcessorNode — deprecated but universally supported.
    // AudioWorklet would be ideal but module loading is fragile in WASM.
    let processor = ctx.create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(1024, 1, 1)?;

    let buffer = Box::new(std::cell::RefCell::new(Vec::<f32>::new()));
    // Raw pointer to the heap-allocated Box target; stays valid after the
    // Box is moved into RxHandle.
    let buf_ptr: *const std::cell::RefCell<Vec<f32>> = &*buffer;

    let on_audio = Closure::<dyn FnMut(web_sys::AudioProcessingEvent)>::new(
        move |event: web_sys::AudioProcessingEvent| {
            let input = match event.input_buffer() {
                Ok(buf) => buf,
                Err(_) => return,
            };
            let samples = match input.get_channel_data(0) {
                Ok(c) => c,
                Err(_) => return,
            };
            unsafe {
                if let Some(b) = buf_ptr.as_ref() {
                    if let Ok(mut b) = b.try_borrow_mut() {
                        b.extend_from_slice(&samples);
                    }
                }
            }
        },
    );
    processor.set_onaudioprocess(Some(on_audio.as_ref().unchecked_ref()));
    on_audio.forget();

    source.connect_with_audio_node(&processor)?;
    // Connect processor to destination to keep the audio graph alive.
    // The processor produces silence on its output by default.
    processor.connect_with_audio_node(&ctx.destination())?;

    let demodulator = OfdmDemodulator::new(config);
    let preamble_samples = config.preamble_samples();
    let frame_samples = config.symbol_duration_samples() * (config.preamble_symbols + config.data_symbols_per_frame);
    let batch_cap = frame_samples * 200;

    Ok(RxHandle {
        demodulator,
        shared_buffer: buffer,
        local_buffer: Vec::new(),
        preamble_samples,
        batch_cap,
        audio_ctx: ctx,
        _processor: processor,
        _source: source,
    })
}

// ── TX ──────────────────────────────────────────────────────────

pub struct TxPlayback {
    _audio_ctx: web_sys::AudioContext,
    _source: web_sys::AudioBufferSourceNode,
    duration_ms: f64,
    _on_done: Option<Closure<dyn FnMut()>>,
}

impl TxPlayback {
    pub fn duration_ms(&self) -> f64 {
        self.duration_ms
    }

    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for TxPlayback {
    fn drop(&mut self) {
        let _ = self._source.stop();
        let _ = self._audio_ctx.close();
    }
}

pub fn play_audio(
    samples: Vec<f32>,
    sample_rate: f32,
    on_complete: impl FnMut() + 'static,
) -> Result<TxPlayback, JsValue> {
    let opts = web_sys::AudioContextOptions::new();
    opts.set_sample_rate(sample_rate);
    let ctx = web_sys::AudioContext::new_with_context_options(&opts)?;
    let _ = ctx.resume();
    let len = samples.len() as u32;
    let num_channels = 1u32;
    let audio_buffer = ctx.create_buffer(num_channels, len, sample_rate)?;
    audio_buffer.copy_to_channel(&samples, 0)?;
    let source = ctx.create_buffer_source()?;
    source.set_buffer(Some(&audio_buffer));
    source.connect_with_audio_node(&ctx.destination())?;
    let done = Closure::<dyn FnMut()>::new(on_complete);
    let callback = done.as_ref().unchecked_ref();
    source.add_event_listener_with_callback("ended", callback)?;
    source.start()?;
    let duration_ms = (len as f64) / (sample_rate as f64) * 1000.0;
    Ok(TxPlayback {
        _audio_ctx: ctx,
        _source: source,
        duration_ms,
        _on_done: Some(done),
    })
}

pub fn play_audio_looped(
    samples: Vec<f32>,
    sample_rate: f32,
) -> Result<TxPlayback, JsValue> {
    let opts = web_sys::AudioContextOptions::new();
    opts.set_sample_rate(sample_rate);
    let ctx = web_sys::AudioContext::new_with_context_options(&opts)?;
    let _ = ctx.resume();
    let len = samples.len() as u32;
    let num_channels = 1u32;
    let audio_buffer = ctx.create_buffer(num_channels, len, sample_rate)?;
    audio_buffer.copy_to_channel(&samples, 0)?;
    let source = ctx.create_buffer_source()?;
    source.set_buffer(Some(&audio_buffer));
    source.set_loop(true);
    source.connect_with_audio_node(&ctx.destination())?;
    source.start()?;
    let duration_ms = (len as f64) / (sample_rate as f64) * 1000.0;
    Ok(TxPlayback {
        _audio_ctx: ctx,
        _source: source,
        duration_ms,
        _on_done: None,
    })
}

pub fn modulate_frame(data: &[u8], config: &Config) -> Vec<f32> {
    let mut modulator = OfdmModulator::new(config);
    let mut audio = modulator.modulate_with_preamble(data);
    audio.extend(std::iter::repeat(0.0f32).take(config.symbol_duration_samples()));
    audio
}

/// Yield to the event loop via Promise.resolve().then() — a microtask.
/// Unlike setTimeout(0) (macrotask), this guarantees pending macrotasks
/// (e.g. click events) are processed before the future resumes.
pub async fn yield_now() {
    let promise = js_sys::Promise::resolve(&JsValue::undefined());
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
}

pub fn build_test_signal(config: &Config, n_frames: usize) -> Vec<f32> {
    use sosw_core::link::frame::FrameAssembler;
    let mut assembler = FrameAssembler::new(config);
    let mut all_audio = Vec::new();

    let guard = config.symbol_duration_samples();

    // 0.5s silence before first frame for capture alignment
    all_audio.extend(std::iter::repeat(0.0f32).take((config.sample_rate as f64 * 0.5) as usize));

    // 3 training frames for consumed_samples stride to converge
    for _ in 0..3 {
        let train = vec![0u8; config.payload_size];
        let frame = assembler.assemble_frame_with_type(&train, 0);
        let mut modulator = OfdmModulator::new(config);
        let samples = modulator.modulate_with_preamble(&frame);
        all_audio.extend_from_slice(&samples);
        all_audio.extend(std::iter::repeat(0.0f32).take(guard));
    }

    for _ in 0..n_frames {
        let test_data: Vec<u8> = (0..config.payload_size)
            .map(|i| (i % 256) as u8)
            .collect();
        let frame = assembler.assemble_frame(&test_data);
        let mut modulator = OfdmModulator::new(config);
        let samples = modulator.modulate_with_preamble(&frame);
        all_audio.extend_from_slice(&samples);
        all_audio.extend(std::iter::repeat(0.0f32).take(guard));
    }
    all_audio
}
