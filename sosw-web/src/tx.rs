use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::audio;

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

    let start_tx = move |_| {
        if playing.get() || file_data.get().is_empty() {
            return;
        }
        playing.set(true);
        progress.set(0.0);
        status.set("Modulating...".to_string());

        let data = file_data.get();
        leptos::task::spawn_local(async move {
            let config = sosw_core::Config::ofdm_default();
            let samples = audio::modulate_frame(&data, &config);
            let sample_rate = 48000.0f32;
            let total_ms = (samples.len() as f64 / sample_rate as f64) * 1000.0;
            status.set(format!("Playing ({:.0} ms)", total_ms));

            let done = {
                let playing = playing.clone();
                let status = status.clone();
                let fname = file_name.get();
                move || {
                    playing.set(false);
                    status.set(format!("Done: {}", fname));
                }
            };

            match audio::play_audio(samples, sample_rate, done) {
                Ok(tx) => {
                    let dur = tx.duration_ms();
                    let interval_ms = (dur / 20.0).max(50.0) as u64;
                    for _ in 0..20 {
                        if !playing.get() { break; }
                        let p = progress.get();
                        progress.set(p + 0.05);
                        gloo_timers::future::sleep(std::time::Duration::from_millis(interval_ms)).await;
                    }
                    tx.stop();
                }
                Err(e) => {
                    status.set(format!("Play error: {:?}", e));
                    playing.set(false);
                }
            }
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
                <button on:click=move |_| { let doc = web_sys::window().and_then(|w| w.document()); if let Some(d) = doc { if let Some(el) = d.get_element_by_id("file-input") { if let Ok(input) = el.dyn_into::<web_sys::HtmlInputElement>() { let html: &web_sys::HtmlElement = input.as_ref(); html.click(); } } } }>
                    "Choose File"
                </button>
                <button
                    on:click=start_tx
                    disabled=move || file_data.get().is_empty() || playing.get()
                >
                    {move || if playing.get() { "Playing..." } else { "Transmit" }}
                </button>
                <span>{move || status.get()}</span>
            </div>
            <div class="stats">
                <div class="stat" style="grid-column: 1 / -1">
                    <label>"Progress"</label>
                    <progress
                        max="20"
                        value=move || (progress.get() * 20.0) as i32
                        style="width:100%"
                    ></progress>
                </div>
            </div>
        </div>
    }
}
