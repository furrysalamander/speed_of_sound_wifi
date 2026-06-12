use leptos::prelude::*;
use wasm_bindgen::prelude::*;

mod audio;

#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Debug);
    mount_to_body(|| view! { <App /> });
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Rx,
    Tx,
}

#[component]
fn App() -> impl IntoView {
    let tab = RwSignal::new(Tab::Rx);

    view! {
        <div class="app">
            <h1>"Speed of Sound WiFi"</h1>
            <div class="tabs">
                <button
                    class="tab"
                    class:active=move || tab.get() == Tab::Rx
                    on:click=move |_| tab.set(Tab::Rx)
                >"RX Demo"</button>
                <button
                    class="tab"
                    class:active=move || tab.get() == Tab::Tx
                    on:click=move |_| tab.set(Tab::Tx)
                >"TX Demo"</button>
            </div>
            {move || match tab.get() {
                Tab::Rx => view! { <RxPanel /> }.into_any(),
                Tab::Tx => view! { <TxPanel /> }.into_any(),
            }}
        </div>
    }
}

#[component]
fn RxPanel() -> impl IntoView {
    let running = RwSignal::new(false);
    let frames = RwSignal::new(0u32);
    let valid_frames = RwSignal::new(0u32);
    let last_peak = RwSignal::new(0.0f32);
    let last_cfo = RwSignal::new(0.0f32);
    let status = RwSignal::new(String::from("Ready"));

    let start_rx = move |_| {
        if running.get() {
            running.set(false);
            status.set("Stopped".to_string());
            return;
        }
        running.set(true);
        status.set("Starting...".to_string());
        leptos::task::spawn_local(async move {
            match audio::start_rx().await {
                Ok(mut rx) => {
                    status.set("Listening...".to_string());
                    while running.get() {
                        if let Some(r) = rx.poll() {
                            frames.update(|n| *n += 1);
                            valid_frames.update(|n| *n += 1);
                            last_peak.set(r.preamble_peak);
                            last_cfo.set(r.cfo_rad_per_sym);
                            status.set(format!("Frame {}", frames.get()));
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
                <button on:click=start_rx>
                    {move || if running.get() { "Stop RX" } else { "Start RX" }}
                </button>
                <span>{move || status.get()}</span>
            </div>
            <div class="stats">
                <div class="stat">
                    <label>"Frames"</label>
                    <span class="val">{move || frames.get()}</span>
                </div>
                <div class="stat">
                    <label>"Valid"</label>
                    <span class="val">{move || valid_frames.get()}</span>
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
            <canvas></canvas>
        </div>
    }
}

#[component]
fn TxPanel() -> impl IntoView {
    let frame_count = RwSignal::new(0u32);
    let status = RwSignal::new(String::from("Upload a file to transmit"));

    let _start_tx = move |_: leptos::ev::MouseEvent| {
        leptos::task::spawn_local(async move {
            status.set("TX started...".to_string());
            status.set("TX complete".to_string());
        });
    };

    view! {
        <div class="panel">
            <div class="row">
                <span>{move || status.get()}</span>
            </div>
            <div class="stats">
                <div class="stat">
                    <label>"Frames sent"</label>
                    <span class="val">{move || frame_count.get()}</span>
                </div>
            </div>
        </div>
    }
}
