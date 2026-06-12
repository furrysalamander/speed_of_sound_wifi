use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::DemodResult;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// ── RX ──────────────────────────────────────────────────────────

pub struct RxHandle {
    demodulator: OfdmDemodulator,
    buffer: std::cell::RefCell<Vec<f32>>,
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
    pub fn poll(&mut self) -> Option<DemodResult> {
        let chunk: Vec<f32> = self.buffer.borrow_mut().drain(..).collect();
        if chunk.len() < 256 {
            return None;
        }
        let result = self.demodulator.process_samples(&chunk);
        self.demodulator.reset();
        result
    }

    pub fn drain_samples(&mut self) -> Vec<f32> {
        self.buffer.borrow_mut().drain(..).collect()
    }

    pub fn stop(self) {
        drop(self);
    }
}

pub async fn start_rx() -> Result<RxHandle, JsValue> {
    let _config = Config::ofdm_default();

    let ctx = web_sys::AudioContext::new()?;

    let window = web_sys::window().ok_or(JsValue::from_str("no window"))?;
    let navigator = window.navigator();
    let media_devices = navigator.media_devices()?;

    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&wasm_bindgen::JsValue::TRUE);

    let promise = media_devices.get_user_media_with_constraints(&constraints)?;
    let stream = wasm_bindgen_futures::JsFuture::from(promise)
        .await?
        .dyn_into::<web_sys::MediaStream>()?;

    let source = ctx.create_media_stream_source(&stream)?;

    let buffer_size = 2048u32;
    let processor = ctx
        .create_script_processor_with_buffer_size_and_number_of_input_channels_and_number_of_output_channels(
            buffer_size, 1, 1,
        )?;

    let buffer = std::cell::RefCell::new(Vec::<f32>::new());

    let buf_ptr = &buffer as *const std::cell::RefCell<Vec<f32>>;

    let closure = Closure::<dyn FnMut(web_sys::AudioProcessingEvent)>::new(
        move |event: web_sys::AudioProcessingEvent| {
            if let Ok(input) = event.input_buffer() {
                if let Ok(data) = input.get_channel_data(0) {
                    unsafe {
                        if let Some(b) = buf_ptr.as_ref() {
                            b.borrow_mut().extend_from_slice(&data);
                        }
                    }
                }
            }
        },
    );

    processor.set_onaudioprocess(Some(closure.as_ref().unchecked_ref()));
    closure.forget();

    source.connect_with_audio_node(&processor)?;
    processor.connect_with_audio_node(&ctx.destination())?;

    let demodulator = OfdmDemodulator::new(&_config);

    Ok(RxHandle {
        demodulator,
        buffer,
        audio_ctx: ctx,
        _processor: processor,
        _source: source,
    })
}

// ── TX ──────────────────────────────────────────────────────────

pub struct TxPlayback {
    audio_ctx: web_sys::AudioContext,
    _source: web_sys::AudioBufferSourceNode,
    duration_ms: f64,
    on_done: Option<Closure<dyn FnMut()>>,
}

impl TxPlayback {
    pub fn duration_ms(&self) -> f64 {
        self.duration_ms
    }

    pub fn stop(self) {
        drop(self);
    }
}

pub fn play_audio(
    samples: Vec<f32>,
    sample_rate: f32,
    on_complete: impl FnMut() + 'static,
) -> Result<TxPlayback, JsValue> {
    let ctx = web_sys::AudioContext::new()?;

    let len = samples.len() as u32;
    let num_channels = 1u32;

    let audio_buffer = ctx.create_buffer(num_channels, len, sample_rate)?;

    audio_buffer.copy_to_channel(&samples, 0)?;

    let source = ctx.create_buffer_source()?;
    source.set_buffer(Some(&audio_buffer));
    source.connect_with_audio_node(&ctx.destination())?;

    let done = Closure::<dyn FnMut()>::new(on_complete);
    source.set_onended(Some(done.as_ref().unchecked_ref()));

    source.start()?;

    let duration_ms = (len as f64) / (sample_rate as f64) * 1000.0;

    Ok(TxPlayback {
        audio_ctx: ctx,
        _source: source,
        duration_ms,
        on_done: Some(done),
    })
}

pub fn modulate_frame(data: &[u8]) -> Vec<f32> {
    let config = Config::ofdm_default();
    let mut modulator = OfdmModulator::new(&config);
    modulator.modulate_with_preamble(data)
}
