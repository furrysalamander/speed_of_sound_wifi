use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::audio;

#[component]
pub fn RxPanel() -> impl IntoView {
    let running = RwSignal::new(false);
    let frames = RwSignal::new(0u32);
    let last_peak = RwSignal::new(0.0f32);
    let last_cfo = RwSignal::new(0.0f32);
    let status = RwSignal::new(String::from("Ready"));

    let waterfall_ref = std::rc::Rc::new(std::cell::RefCell::new(None::<crate::waterfall::Waterfall>));

    let start_rx = move |_| {
        if running.get() {
            running.set(false);
            status.set("Stopped".to_string());
            return;
        }
        running.set(true);
        status.set("Starting...".to_string());

        let doc = web_sys::window().and_then(|w| w.document());
        let canvas = doc.and_then(|d| d.get_element_by_id("waterfall-rx"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok());

        if let Some(c) = canvas {
            *waterfall_ref.borrow_mut() = crate::waterfall::Waterfall::new(c).ok();
        }

        let wf = waterfall_ref.clone();
        leptos::task::spawn_local(async move {
            let config = sosw_core::Config::ofdm_default();
            match audio::start_rx(&config).await {
                Ok(mut rx) => {
                    status.set("Listening...".to_string());
                    while running.get_untracked() {
                        let (samples, result_opt) = rx.poll();
                        if !samples.is_empty() {
                            if let Some(ref mut w) = *wf.borrow_mut() {
                                let spec = crate::waterfall::compute_spectrum(&samples, 256);
                                w.push_spectrum(&spec);
                                w.render();
                            }
                        }
                        if let Some(r) = result_opt {
                            frames.update(|n| *n += 1);
                            last_peak.set(r.preamble_peak);
                            last_cfo.set(r.cfo_rad_per_sym);
                            status.set(format!("Frame {}", frames.get_untracked()));
                        }
                        gloo_timers::future::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    rx.stop();
                }
                Err(e) => {
                    status.set(format!("Error: {:?}", e));
                    running.set(false);
                }
            }
        });
    };

    view! {
        <div class="panel">
            <div class="row">
                <button class="btn" class:btn-stop=move || running.get() on:click=start_rx>
                    {move || if running.get() { "Stop RX" } else { "Start RX" }}
                </button>
                <span class="status">{move || status.get()}</span>
            </div>
            <div class="stats">
                <div class="stat">
                    <label>"Frames"</label>
                    <span class="val">{move || frames.get()}</span>
                </div>
                <div class="stat">
                    <label>"Preamble Peak"</label>
                    <span class="val">{move || format!("{:.3}", last_peak.get())}</span>
                </div>
                <div class="stat">
                    <label>"CFO (rad/sym)"</label>
                    <span class="val">{move || format!("{:.4}", last_cfo.get())}</span>
                </div>
            </div>
            <canvas id="waterfall-rx" height="200" width="512"></canvas>
        </div>
    }
}
