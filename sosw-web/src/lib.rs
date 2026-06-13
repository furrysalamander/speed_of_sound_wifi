use leptos::prelude::*;

mod audio;
mod presets;
mod rx;
mod tx;
mod waterfall;

mod debug;

#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    _ = console_log::init_with_level(log::Level::Debug);
    mount_to_body(|| view! { <App /> });
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Rx,
    Tx,
    Debug,
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
                >"RX"</button>
                <button
                    class="tab"
                    class:active=move || tab.get() == Tab::Tx
                    on:click=move |_| tab.set(Tab::Tx)
                >"TX"</button>
                <button
                    class="tab"
                    class:active=move || tab.get() == Tab::Debug
                    on:click=move |_| tab.set(Tab::Debug)
                >"Debug"</button>
            </div>
            <div style:display=move || if tab.get() == Tab::Rx { "block" } else { "none" }>
                <rx::RxPanel />
            </div>
            <div style:display=move || if tab.get() == Tab::Tx { "block" } else { "none" }>
                <tx::TxPanel />
            </div>
            <div style:display=move || if tab.get() == Tab::Debug { "block" } else { "none" }>
                <debug::DebugPanel />
            </div>
        </div>
    }
}
