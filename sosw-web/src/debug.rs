use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use sosw_core::Config;
use sosw_core::link::frame::FrameParser;

use crate::audio;
use crate::waterfall;
use crate::presets;

// ── DebugPanel ──────────────────────────────────────────────────

#[component]
pub fn DebugPanel() -> impl IntoView {
    let config = RwSignal::new(presets::load_saved_config());
    let rx_running = RwSignal::new(false);
    let tx_active = RwSignal::new(false);
    let rx_status = RwSignal::new(String::from("Ready"));
    let tx_status = RwSignal::new(String::from("Ready"));

    // Stats
    let frames_total = RwSignal::new(0u32);
    let frames_valid = RwSignal::new(0u32);
    let frames_dropped = RwSignal::new(0u32);
    let last_peak = RwSignal::new(0.0f32);
    let last_cfo = RwSignal::new(0.0f32);
    let last_h_mean = RwSignal::new(0.0f32);
    let crc_fails = RwSignal::new(0u32);
    let fec_fails = RwSignal::new(0u32);
    let frame_rate = RwSignal::new(0.0f32);
    let per_sc = RwSignal::new(Vec::<f32>::new());

    // Save config whenever it changes
    Effect::new(move |_| {
        let c = config.get();
        presets::save_config(&c);
    });

    view! {
        <div class="panel">
            <ConfigSection config />
            <MonitorSection
                config
                rx_running
                frames_total
                frames_valid
                frames_dropped
                last_peak
                last_cfo
                last_h_mean
                crc_fails
                fec_fails
                frame_rate
                per_sc
                rx_status
            />
            <LoopbackSection
                config
                tx_active
                tx_status
            />
        </div>
    }
}

// ── Config Section ──────────────────────────────────────────────

#[component]
fn ConfigSection(
    config: RwSignal<Config>,
) -> impl IntoView {
    let apply_preset = move |maker: fn() -> Config| {
        config.set(maker());
    };

    let set_sc_min = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).parse::<usize>().unwrap_or(1);
        let mut c = config.get();
        c.sc_min = v.clamp(1, c.sc_max.saturating_sub(1));
        config.set(c);
    };

    let set_sc_max = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).parse::<usize>().unwrap_or(1);
        let mut c = config.get();
        c.sc_max = v.clamp(c.sc_min + 1, c.fft_size / 2 - 1);
        config.set(c);
    };

    let set_fft = move |fft: usize| {
        let mut c = config.get();
        c.fft_size = fft;
        c.sc_max = c.sc_max.min(fft / 2 - 1);
        config.set(c);
    };

    let set_cp = move |cp: usize| {
        let mut c = config.get();
        c.cp_length = cp;
        config.set(c);
    };

    let set_thresh = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).parse::<f32>().unwrap_or(0.05);
        let mut c = config.get();
        c.preamble_threshold = v.clamp(0.01, 0.50);
        config.set(c);
    };

    let set_amp = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).parse::<f32>().unwrap_or(0.08);
        let mut c = config.get();
        c.output_amplitude = v.clamp(0.01, 1.0);
        config.set(c);
    };

    let set_pll = move |ev: leptos::ev::Event| {
        let v = event_target_value(&ev).parse::<f32>().unwrap_or(0.08);
        let mut c = config.get();
        c.pll_beta = v.clamp(0.01, 0.50);
        config.set(c);
    };

    let derived = move || {
        let c = config.get();
        let fs = c.sample_rate as f64;
        let bin_w = fs / c.fft_size as f64;
        let f_min = c.sc_min as f64 * bin_w;
        let f_max = c.sc_max as f64 * bin_w;
        let sym_rate = fs / (c.fft_size + c.cp_length) as f64;
        let bps = c.active_subcarriers() as f64 * 2.0 * sym_rate;
        let preset_name = presets::which_preset(&c);
        format!("{} | {:.0}–{:.0} Hz | {} SCs | {:.0} sym/s | {:.0} bps",
                preset_name, f_min, f_max, c.active_subcarriers(), sym_rate, bps)
    };

    let sc_max_limit = move || (config.get().fft_size / 2 - 1).to_string();
    let thresh_str = move || format!("{:.2}", config.get().preamble_threshold);
    let amp_str = move || format!("{:.2}", config.get().output_amplitude);
    let pll_str = move || format!("{:.2}", config.get().pll_beta);

    view! {
        <details open>
            <summary style="cursor:pointer;font-weight:bold;color:#4fc3f7;">
                "Config"
            </summary>
            <div class="row" style="gap:4px;flex-wrap:wrap;">
                {move || presets::PRESETS.iter().map(|p| {
                    let maker = p.maker;
                    let name = p.name;
                    view! {
                        <button
                            class="btn small"
                            on:click=move |_| apply_preset(maker)
                        >
                            {name}
                        </button>
                    }
                }).collect::<Vec<_>>()}
            </div>
            <div class="row">
                <label style="font-size:0.8rem;min-width:60px;">
                    "SC min: " {move || config.get().sc_min.to_string()}
                </label>
                <input type="range" min="1" max="255"
                    prop:value=move || config.get().sc_min.to_string()
                    on:input=set_sc_min style="flex:1" />
            </div>
            <div class="row">
                <label style="font-size:0.8rem;min-width:60px;">
                    "SC max: " {move || config.get().sc_max.to_string()}
                </label>
                <input type="range" min="2" max=sc_max_limit
                    prop:value=move || config.get().sc_max.to_string()
                    on:input=set_sc_max style="flex:1" />
            </div>
            <div class="row" style="gap:4px;">
                <span style="font-size:0.8rem;">"FFT:"</span>
                {move || [128usize, 256, 512].into_iter().map(|v| {
                    let active = config.get().fft_size == v;
                    view! {
                        <button
                            class="btn small"
                            class:active=move || active
                            on:click=move |_| set_fft(v)
                        >{v}</button>
                    }
                }).collect::<Vec<_>>()}
                <span style="font-size:0.8rem;margin-left:8px;">"CP:"</span>
                {move || [16usize, 32, 64, 128].into_iter().map(|v| {
                    let active = config.get().cp_length == v;
                    view! {
                        <button
                            class="btn small"
                            class:active=move || active
                            on:click=move |_| set_cp(v)
                        >{v}</button>
                    }
                }).collect::<Vec<_>>()}
            </div>
            <div class="row">
                <label style="font-size:0.8rem;min-width:100px;">
                    "Thresh: " {move || format!("{:.2}", config.get().preamble_threshold)}
                </label>
                <input type="range" min="0.01" max="0.50" step="0.01"
                    prop:value=thresh_str
                    on:input=set_thresh style="flex:1" />
            </div>
            <div class="row">
                <label style="font-size:0.8rem;min-width:100px;">
                    "TX Amp: " {move || format!("{:.2}", config.get().output_amplitude)}
                </label>
                <input type="range" min="0.01" max="1.00" step="0.01"
                    prop:value=amp_str
                    on:input=set_amp style="flex:1" />
            </div>
            <div class="row">
                <label style="font-size:0.8rem;min-width:100px;">
                    "PLL β: " {move || format!("{:.2}", config.get().pll_beta)}
                </label>
                <input type="range" min="0.01" max="0.50" step="0.01"
                    prop:value=pll_str
                    on:input=set_pll style="flex:1" />
            </div>
            <div class="row">
                <span style="font-size:0.8rem;color:#888;">{derived}</span>
            </div>
        </details>
    }
}

// ── Monitor Section ─────────────────────────────────────────────

#[component]
fn MonitorSection(
    config: RwSignal<Config>,
    rx_running: RwSignal<bool>,
    frames_total: RwSignal<u32>,
    frames_valid: RwSignal<u32>,
    frames_dropped: RwSignal<u32>,
    last_peak: RwSignal<f32>,
    last_cfo: RwSignal<f32>,
    last_h_mean: RwSignal<f32>,
    crc_fails: RwSignal<u32>,
    fec_fails: RwSignal<u32>,
    frame_rate: RwSignal<f32>,
    per_sc: RwSignal<Vec<f32>>,
    rx_status: RwSignal<String>,
) -> impl IntoView {
    let wf_ref = std::rc::Rc::new(std::cell::RefCell::new(None::<waterfall::Waterfall>));
    let per_sc_canvas = std::rc::Rc::new(std::cell::RefCell::new(None::<web_sys::HtmlCanvasElement>));

    let start_stop = move |_| {
        if rx_running.get_untracked() {
            rx_running.set(false);
            rx_status.set("Stopped".to_string());
            return;
        }

        rx_running.set(true);
        rx_status.set("Starting...".to_string());

        // Reset stats
        frames_total.set(0);
        frames_valid.set(0);
        frames_dropped.set(0);
        last_peak.set(0.0);
        last_cfo.set(0.0);
        last_h_mean.set(0.0);
        crc_fails.set(0);
        fec_fails.set(0);
        frame_rate.set(0.0);
        per_sc.set(Vec::new());

        let doc = web_sys::window().and_then(|w| w.document());

        // Waterfall canvas
        let canvas = doc.as_ref()
            .and_then(|d| d.get_element_by_id("debug-waterfall"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok());
        if let Some(c) = canvas {
            *wf_ref.borrow_mut() = waterfall::Waterfall::new(c).ok();
        }

        // Per-subcarrier canvas
        let psc = doc.as_ref()
            .and_then(|d| d.get_element_by_id("debug-psc"))
            .and_then(|el| el.dyn_into::<web_sys::HtmlCanvasElement>().ok());
        *per_sc_canvas.borrow_mut() = psc;

        let cfg = config.get();
        let wf = wf_ref.clone();
        let psc_ref = per_sc_canvas.clone();
        let running = rx_running.clone();
        let st = rx_status.clone();
        let ft = frames_total.clone();
        let fv = frames_valid.clone();
        let fd = frames_dropped.clone();
        let lp = last_peak.clone();
        let lc = last_cfo.clone();
        let lh = last_h_mean.clone();
        let crc = crc_fails.clone();
        let fec = fec_fails.clone();
        let fr = frame_rate.clone();
        let ps = per_sc.clone();

        leptos::task::spawn_local(async move {
            // Update waterfall band info
            {
                let mut wf_borrow = wf.borrow_mut();
                if let Some(ref mut w) = *wf_borrow {
                    w.set_active_band(cfg.sc_min, cfg.sc_max, cfg.fft_size, cfg.sample_rate);
                }
            }

            match audio::start_rx(&cfg).await {
                Ok(mut rx) => {
                    st.set("Listening...".to_string());
                    let mut frame_parser = FrameParser::new(&cfg);
                    let mut last_frame_time = wasm_bindgen::JsValue::NULL;
                    let mut frame_count_last = 0u32;

                    while running.get_untracked() {
                        let (samples, result_opt) = rx.poll();
                        if !samples.is_empty() {
                            let mut wf_borrow = wf.borrow_mut();
                            if let Some(ref mut w) = *wf_borrow {
                                let spec = waterfall::compute_spectrum(&samples, cfg.fft_size);
                                w.push_spectrum(&spec);
                                w.render();
                            }
                            drop(wf_borrow);
                        }

                        // Demodulate
                        if let Some(result) = result_opt {
                            ft.update(|n| *n += 1);
                            lp.set(result.preamble_peak);
                            lc.set(result.cfo_rad_per_sym);
                            lh.set(result.mean_h_magnitude);
                            ps.set(result.per_sc_h.clone());

                            // Feed through FrameParser for CRC/FEC validation
                            let parsed = frame_parser.feed_bytes(&result.bytes);
                            for pf in &parsed {
                                if pf.valid {
                                    fv.update(|n| *n += 1);
                                }
                            }
                            if !parsed.is_empty() && !parsed.iter().any(|p| p.valid) {
                                fd.update(|n| *n += 1);
                            }
                            crc.set(frame_parser.crc_fail as u32);
                            fec.set(frame_parser.fec_fail as u32);

                            // Frame rate
                            let now = js_sys::Date::now();
                            if last_frame_time != wasm_bindgen::JsValue::NULL {
                                let elapsed_s = (now - last_frame_time.as_f64().unwrap_or(0.0)) / 1000.0;
                                if elapsed_s > 0.0 {
                                    let rate_val = (ft.get_untracked() - frame_count_last) as f32 / elapsed_s as f32;
                                    fr.set(rate_val);
                                }
                            }
                            last_frame_time = JsValue::from_f64(now);
                            frame_count_last = ft.get_untracked();

                            st.set(format!("Frame {}", ft.get_untracked()));
                        }

                        // Draw per-subcarrier bar chart
                        let h_values = ps.get_untracked();
                        if !h_values.is_empty() {
                            draw_per_sc_chart(&psc_ref, &h_values, &cfg);
                        }

                        gloo_timers::future::sleep(std::time::Duration::from_millis(50)).await;
                    }
                    rx.stop();
                }
                Err(e) => {
                    st.set(format!("Error: {:?}", e));
                    running.set(false);
                }
            }
        });
    };

    view! {
        <details open>
            <summary style="cursor:pointer;font-weight:bold;color:#4fc3f7;margin-top:8px;">
                "RX Monitor"
            </summary>
            <div class="row">
                <button
                    class="btn"
                    class:btn-stop=move || rx_running.get()
                    on:click=start_stop
                >
                    {move || if rx_running.get() { "Stop RX" } else { "Start RX" }}
                </button>
                <span class="status">{move || rx_status.get()}</span>
            </div>
            <div class="stats" style="grid-template-columns:1fr 1fr 1fr;">
                <div class="stat">
                    <label>"Frames"</label>
                    <span class="val">{move || frames_total.get()}</span>
                </div>
                <div class="stat">
                    <label>"Valid"</label>
                    <span class="val">{move || frames_valid.get()}</span>
                </div>
                <div class="stat">
                    <label>"Dropped"</label>
                    <span class="val">{move || frames_dropped.get()}</span>
                </div>
                <div class="stat">
                    <label>"Peak"</label>
                    <span class="val">{move || format!("{:.3}", last_peak.get())}</span>
                </div>
                <div class="stat">
                    <label>"CFO (rad/sym)"</label>
                    <span class="val">{move || format!("{:.4}", last_cfo.get())}</span>
                </div>
                <div class="stat">
                    <label>"|H| mean"</label>
                    <span class="val">{move || format!("{:.4}", last_h_mean.get())}</span>
                </div>
                <div class="stat">
                    <label>"FPS"</label>
                    <span class="val">{move || format!("{:.1}", frame_rate.get())}</span>
                </div>
                <div class="stat">
                    <label>"CRC fail"</label>
                    <span class="val">{move || crc_fails.get()}</span>
                </div>
                <div class="stat">
                    <label>"FEC fail"</label>
                    <span class="val">{move || fec_fails.get()}</span>
                </div>
            </div>
            <canvas id="debug-psc" height="48" width="512"
                style="width:100%;height:48px;background:#111;border-radius:4px;margin:4px 0;">
            </canvas>
            <canvas id="debug-waterfall" height="160" width="512"
                style="width:100%;height:160px;background:#000;border-radius:4px;margin:4px 0;">
            </canvas>
        </details>
    }
}

fn draw_per_sc_chart(
    canvas_ref: &std::rc::Rc<std::cell::RefCell<Option<web_sys::HtmlCanvasElement>>>,
    h_values: &[f32],
    config: &Config,
) {
    let canvas = match *canvas_ref.borrow() {
        Some(ref c) => c.clone(),
        None => return,
    };
    let ctx: web_sys::CanvasRenderingContext2d = match canvas.get_context("2d") {
        Ok(Some(c)) => c.unchecked_into(),
        _ => return,
    };

    let w = canvas.width() as f64;
    let h = canvas.height() as f64;

    // Clear
    ctx.set_fill_style_str("#111");
    ctx.fill_rect(0.0, 0.0, w, h);

    if h_values.is_empty() {
        return;
    }

    let max_h = h_values.iter().fold(1e-6f32, |a, &b| a.max(b));
    let n = h_values.len();
    let bar_w = w / n as f64;

    for (i, &val) in h_values.iter().enumerate() {
        let bar_h = (val / max_h) as f64 * (h - 2.0);
        let x = i as f64 * bar_w;
        let y = h - bar_h;

        // Color: green for strong, yellow for moderate, red for weak
        let ratio = val / max_h;
        let (r, g, b) = if ratio > 0.7 {
            (0, 200, 80)
        } else if ratio > 0.3 {
            (200, 200, 0)
        } else {
            (200, 50, 0)
        };
        let color = format!("rgb({},{},{})", r, g, b);
        ctx.set_fill_style_str(&color);
        ctx.fill_rect(x, y, bar_w.max(1.0), bar_h);
    }

    // Draw band edge markers
    let half_bins = config.fft_size / 2;
    let sc_min_x = config.sc_min as f64 * w / half_bins as f64;
    let sc_max_x = (config.sc_max + 1) as f64 * w / half_bins as f64;
    ctx.set_stroke_style_str("#4fc3f7");
    ctx.set_line_width(1.0);
    ctx.begin_path();
    ctx.move_to(sc_min_x, 0.0);
    ctx.line_to(sc_min_x, h);
    ctx.stroke();
    ctx.begin_path();
    ctx.move_to(sc_max_x, 0.0);
    ctx.line_to(sc_max_x, h);
    ctx.stroke();
}

// ── Loopback / Test Section ─────────────────────────────────────

#[component]
fn LoopbackSection(
    config: RwSignal<Config>,
    tx_active: RwSignal<bool>,
    tx_status: RwSignal<String>,
) -> impl IntoView {
    let frames_sent = RwSignal::new(0u32);
    let frames_received = RwSignal::new(0u32);
    let tx_handle = std::rc::Rc::new(std::cell::RefCell::new(None::<audio::TxPlayback>));

    let tx_handle_clone = tx_handle.clone();
    let start_test = move |_| {
        if tx_active.get_untracked() {
            tx_active.set(false);
            tx_status.set("TX stopped".to_string());
            *tx_handle_clone.borrow_mut() = None;
            return;
        }

        tx_active.set(true);
        tx_status.set("Starting test TX...".to_string());

        let cfg = config.get();
        let n_frames = 20usize;
        let total = n_frames as u32;
        frames_sent.set(total);
        let txh = tx_handle_clone.clone();

        leptos::task::spawn_local(async move {
            let samples = audio::build_test_signal(&cfg, n_frames);
            let sample_rate = cfg.sample_rate as f32;
            let tx = audio::play_audio_looped(samples, sample_rate);
            match tx {
                Ok(t) => {
                    tx_status.set(format!("TX: {} frames (looped)", total));
                    *txh.borrow_mut() = Some(t);
                }
                Err(e) => {
                    tx_status.set(format!("TX error: {:?}", e));
                    tx_active.set(false);
                }
            }
        });
    };

    view! {
        <details open>
            <summary style="cursor:pointer;font-weight:bold;color:#4fc3f7;margin-top:8px;">
                "Test / Loopback"
            </summary>
            <div class="row">
                <button
                    class="btn"
                    class:btn-stop=move || tx_active.get()
                    on:click=start_test
                >
                    {move || if tx_active.get() { "Stop TX" } else { "Start Test TX" }}
                </button>
                <span class="status">{move || tx_status.get()}</span>
            </div>
            <div class="stats" style="grid-template-columns:1fr 1fr;">
                <div class="stat">
                    <label>"Frames Sent"</label>
                    <span class="val">{move || frames_sent.get()}</span>
                </div>
                <div class="stat">
                    <label>"Frames Received"</label>
                    <span class="val">{move || frames_received.get()}</span>
                </div>
            </div>
            <p style="font-size:0.8rem;color:#888;margin-top:4px;">
                "Start loopback test: press Start TX to begin continuous transmission, "
                "then press Start RX in panel above to receive. Use the same device "
                "or two devices — works with both."
            </p>
        </details>
    }
}
