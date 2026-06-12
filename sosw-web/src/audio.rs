use sosw_core::config::Config;
use sosw_core::physical::ofdm_demod::DemodResult;
use sosw_core::physical::ofdm_demod::OfdmDemodulator;
use sosw_core::physical::ofdm_mod::OfdmModulator;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// ── RX ──────────────────────────────────────────────────────────

const WORKLET_JS: &str = r#"
class SoswRxProcessor extends AudioWorkletProcessor {
    constructor() { super(); }
    process(inputs, outputs, parameters) {
        const input = inputs[0];
        if (input && input[0] && input[0] instanceof Float32Array) {
            this.port.postMessage(input[0], [input[0].buffer]);
        }
        return true;
    }
}
registerProcessor('sosw-rx', SoswRxProcessor);
"#;

pub struct RxHandle {
    demodulator: OfdmDemodulator,
    buffer: std::cell::RefCell<Vec<f32>>,
    audio_ctx: web_sys::AudioContext,
    _worklet: web_sys::AudioWorkletNode,
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

pub async fn start_rx(config: &Config) -> Result<RxHandle, JsValue> {
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

    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(WORKLET_JS));
    let blob = web_sys::Blob::new_with_str_sequence(&parts)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)?;
    let module_promise = ctx.audio_worklet()?.add_module(&url)?;
    wasm_bindgen_futures::JsFuture::from(module_promise).await?;
    web_sys::Url::revoke_object_url(&url)?;

    let buffer = std::cell::RefCell::new(Vec::<f32>::new());
    let buf_ptr = &buffer as *const std::cell::RefCell<Vec<f32>>;

    let options = {
        #[allow(unused_mut)]
        let mut opt = web_sys::AudioWorkletNodeOptions::new();
        opt.set_number_of_inputs(1);
        opt.set_number_of_outputs(0);
        opt
    };
    let worklet = web_sys::AudioWorkletNode::new_with_options(
        ctx.unchecked_ref::<web_sys::BaseAudioContext>(),
        "sosw-rx",
        &options,
    )?;

    let on_msg = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(
        move |event: web_sys::MessageEvent| {
            if let Some(data) = event.data().dyn_into::<js_sys::Float32Array>().ok() {
                unsafe {
                    if let Some(b) = buf_ptr.as_ref() {
                        let mut b = b.borrow_mut();
                        let len = data.length() as usize;
                        let start = b.len();
                        b.resize(start + len, 0.0);
                        data.copy_to(&mut b[start..]);
                    }
                }
            }
        },
    );
    worklet.port()?.set_onmessage(Some(on_msg.as_ref().unchecked_ref()));
    on_msg.forget();

    source.connect_with_audio_node(&worklet)?;

    let demodulator = OfdmDemodulator::new(config);

    Ok(RxHandle {
        demodulator,
        buffer,
        audio_ctx: ctx,
        _worklet: worklet,
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
    let ctx = web_sys::AudioContext::new()?;
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
    // No on_complete for looped playback
    Ok(TxPlayback {
        _audio_ctx: ctx,
        _source: source,
        duration_ms,
        _on_done: None,
    })
}

pub fn modulate_frame(data: &[u8], config: &Config) -> Vec<f32> {
    let mut modulator = OfdmModulator::new(config);
    modulator.modulate_with_preamble(data)
}

pub fn build_test_signal(config: &Config, n_frames: usize) -> Vec<f32> {
    use sosw_core::link::frame::FrameAssembler;
    let mut assembler = FrameAssembler::new(config);
    let mut all_audio = Vec::new();
    for _ in 0..n_frames {
        let test_data: Vec<u8> = (0..config.payload_size)
            .map(|i| (i % 256) as u8)
            .collect();
        let frame = assembler.assemble_frame(&test_data);
        let mut modulator = OfdmModulator::new(config);
        let samples = modulator.modulate_with_preamble(&frame);
        all_audio.extend_from_slice(&samples);
    }
    all_audio
}
