use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use std::cell::RefCell;
use std::rc::Rc;
use sosw_core::link::frame::FrameAssembler;
use sosw_core::physical::ofdm_mod::OfdmModulator;

async fn read_file_as_bytes(file: web_sys::File) -> Result<Vec<u8>, JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let reader = match web_sys::FileReader::new() {
            Ok(r) => r,
            Err(e) => { let _ = reject.call1(&JsValue::NULL, &e); return; }
        };
        let r_clone = reader.clone();
        let onload = Closure::<dyn FnMut()>::new(move || {
            match r_clone.result() {
                Ok(val) => { let _ = resolve.call1(&JsValue::NULL, &val); }
                Err(e) => { let _ = reject.call1(&JsValue::NULL, &e); }
            }
        });
        reader.set_onloadend(Some(onload.as_ref().unchecked_ref()));
        onload.forget();
        let _ = reader.read_as_array_buffer(&file);
    });
    let buf = wasm_bindgen_futures::JsFuture::from(promise).await?;
    let uint8 = js_sys::Uint8Array::new(&buf);
    let mut bytes = vec![0u8; uint8.length() as usize];
    uint8.copy_to(&mut bytes);
    Ok(bytes)
}

#[component]
pub fn TxPanel() -> impl IntoView {
    let status = RwSignal::new(String::from("Select a file to transmit"));
    let progress = RwSignal::new(0.0f32);
    let playing = RwSignal::new(false);
    let file_data = RwSignal::new(Vec::<u8>::new());
    let file_name = RwSignal::new(String::new());
    // Shared cell holding the AudioContext so the stop handler can close it
    // synchronously, bypassing any event-loop timing issues.
    let tx_ctx: Rc<RefCell<Option<web_sys::AudioContext>>> = Rc::new(RefCell::new(None));
    let tx_ctx_stop = tx_ctx.clone();

    let on_file_select = move |_| {
        let document = web_sys::window().and_then(|w| w.document());
        if let Some(doc) = document {
            if let Some(input) = doc.get_element_by_id("file-input") {
                if let Ok(input) = input.dyn_into::<web_sys::HtmlInputElement>() {
                    if let Some(files) = input.files() {
                        if let Some(file) = files.get(0) {
                            let fname = file.name();
                            status.set(format!("Loading: {}", fname));
                            file_name.set(fname.clone());
                            leptos::task::spawn_local({
                                let status = status.clone();
                                let file_data = file_data.clone();
                                async move {
                                    match read_file_as_bytes(file).await {
                                        Ok(bytes) => {
                                            file_data.set(bytes.clone());
                                            status.set(format!("Loaded {} ({} bytes)", fname, bytes.len()));
                                        }
                                        Err(e) => {
                                            status.set(format!("Read error: {:?}", e));
                                        }
                                    }
                                }
                            });
                        }
                    }
                }
            }
        }
    };

    let start_stop = move |_| {
        if playing.get_untracked() {
            // Synchronously close the AudioContext → kills all scheduled audio
            // immediately, and causes the async task to break on the next
            // fallible Web Audio call.
            if let Some(ctx) = tx_ctx_stop.borrow_mut().take() {
                let _ = ctx.close();
            }
            playing.set(false);
            status.set("Stopped".to_string());
            return;
        }

        let data = file_data.get_untracked();
        if data.is_empty() {
            return;
        }

        playing.set(true);
        progress.set(0.0);
        let fname = file_name.get_untracked();
        status.set(format!("Starting: {}", fname));

        let ctx_handle = tx_ctx.clone();

        leptos::task::spawn_local(async move {
            let config = sosw_core::Config::ofdm_default();
            let opts = web_sys::AudioContextOptions::new();
            opts.set_sample_rate(48000.0);
            let ctx = match web_sys::AudioContext::new_with_context_options(&opts) {
                Ok(c) => c,
                Err(e) => {
                    status.set(format!("Audio error: {:?}", e));
                    playing.set(false);
                    return;
                }
            };
            // Publish so the stop handler can close it synchronously.
            *ctx_handle.borrow_mut() = Some(ctx.clone());

            let payload_size = config.payload_size;
            let frame_dur = config.frame_samples() as f64 / config.sample_rate as f64;
            let n_frames = (data.len() + payload_size - 1) / payload_size;
            let total = n_frames;
            let mut assembler = FrameAssembler::new(&config);
            status.set(format!("0/{}", total));
            let t0 = ctx.current_time();

            for i in 0..n_frames {
                // Check both the leptos signal and the ctx handle (in case
                // the stop handler closed the context mid-iteration).
                if !playing.get_untracked() { break; }
                if ctx_handle.borrow().is_none() { break; }

                // Yield via macrotask (setTimeout(0)) so the Stop click
                // handler can run before the next frame is scheduled.
                gloo_timers::future::sleep(std::time::Duration::from_millis(0)).await;

                if !playing.get_untracked() { break; }
                if ctx_handle.borrow().is_none() { break; }

                let start = i * payload_size;
                let end = (start + payload_size).min(data.len());
                let chunk = &data[start..end];

                let frame = assembler.assemble_frame(chunk);
                let mut modulator = OfdmModulator::new(&config);
                let samples = modulator.modulate_with_preamble(&frame);

                let buffer = match ctx.create_buffer(1, samples.len() as u32, config.sample_rate as f32) {
                    Ok(b) => b,
                    Err(_) => { break; }
                };
                if buffer.copy_to_channel(&samples, 0).is_err() { break; }

                let source = match ctx.create_buffer_source() {
                    Ok(s) => s,
                    Err(_) => { break; }
                };
                source.set_buffer(Some(&buffer));
                if source.connect_with_audio_node(&ctx.destination()).is_err() { break; }
                if source.start_with_when(t0 + i as f64 * frame_dur).is_err() { break; }

                progress.set((i + 1) as f32 / total as f32);
                status.set(format!("{}/{}", i + 1, total));
            }

            // Wait for playback to finish (or stop signal)
            let play_end = t0 + n_frames as f64 * frame_dur;
            loop {
                if !playing.get_untracked() { break; }
                if ctx_handle.borrow().is_none() { break; }
                let now = ctx.current_time();
                if now >= play_end { break; }
                gloo_timers::future::sleep(std::time::Duration::from_millis(100)).await;
            }

            // Clean up
            *ctx_handle.borrow_mut() = None;
            if playing.get_untracked() {
                playing.set(false);
                progress.set(1.0);
                status.set(format!("Done: {}", fname));
            }
            drop(ctx);
        });
    };

    view! {
        <div class="panel">
            <div class="row">
                <input
                    id="file-input"
                    type="file"
                    on:change=on_file_select
                    style="display:none"
                />
                <button
                    class="btn"
                    on:click=move |_| {
                        let doc = web_sys::window().and_then(|w| w.document());
                        if let Some(d) = doc {
                            if let Some(el) = d.get_element_by_id("file-input") {
                                if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() {
                                    let html: &web_sys::HtmlElement = input.as_ref();
                                    html.click();
                                }
                            }
                        }
                    }
                >
                    "Choose File"
                </button>
                <button
                    class="btn"
                    class:btn-stop=move || playing.get()
                    on:click=start_stop
                >
                    {move || if playing.get() { "Stop" } else { "Transmit" }}
                </button>
                <span class="status">{move || status.get()}</span>
            </div>
            <div class="stats">
                <div class="stat" style="grid-column: 1 / -1">
                    <label>"Progress"</label>
                    <progress
                        max="100"
                        value=move || (progress.get() * 100.0) as i32
                        style="width:100%"
                    ></progress>
                </div>
            </div>
        </div>
    }
}
