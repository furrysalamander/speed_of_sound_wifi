use anyhow::Result;
use clap::Parser;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use sosw_core::Config;
use sosw_tap::mac::{Mac, MacConfig};
use sosw_tap::phy::Phy;
use sosw_tap::tap::{Tappable, TapInterface};

#[derive(Parser)]
#[command(name = "sosw-tap", about = "Sonic WiFi — Ethernet over OFDM audio")]
enum Cli {
    /// Start a sonic Ethernet node
    Serve {
            /// TAP interface name
            #[arg(long, default_value = "sosw0")]
            tap: String,

            /// PHY preset (default, high_baud, robust, ultrasonic, ultrawide)
            #[arg(short = 'p', long, default_value = "default")]
            preset: String,

            /// Node ID (0-255); derived from MAC if not set
            #[arg(long)]
            node_id: Option<u8>,

        /// Explicit MAC address
        #[arg(long)]
        mac: Option<String>,

        /// Audio output device name
        #[arg(long)]
        tx_device: Option<String>,

        /// Audio input device name
        #[arg(long)]
        rx_device: Option<String>,

        /// DIFS in milliseconds
        #[arg(long, default_value = "600")]
        difs: u64,

        /// Slot time in milliseconds
        #[arg(long, default_value = "60")]
        slot: u64,

        /// Minimum contention window (slots)
        #[arg(long, default_value = "4")]
        cw_min: u8,

        /// Maximum contention window (slots)
        #[arg(long, default_value = "64")]
        cw_max: u8,

        /// Max retries per fragment
        #[arg(long, default_value = "5")]
        max_retries: u8,

        /// ACK timeout in milliseconds
        #[arg(long, default_value = "2000")]
        ack_timeout: u64,
    },
    /// List available audio devices
    ListDevices,
}

fn main() -> Result<()> {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .init();

    match Cli::parse() {
        Cli::ListDevices => list_devices(),
        Cli::Serve {
            tap,
            preset,
            node_id,
            mac: _mac,
            tx_device,
            rx_device,
            difs,
            slot,
            cw_min,
            cw_max,
            max_retries,
            ack_timeout,
        } => serve(
            &tap,
            &preset,
            node_id,
            tx_device.as_deref(),
            rx_device.as_deref(),
            difs,
            slot,
            cw_min,
            cw_max,
            max_retries,
            ack_timeout,
        ),
    }
}

fn list_devices() -> Result<()> {
    let host = cpal::default_host();
    println!("=== Output devices ===");
    for dev in host.output_devices()? {
        let label = dev.id().map(|id| format!("{}", id)).unwrap_or_else(|_| "?".into());
        println!("  {}", label);
    }
    println!("=== Input devices ===");
    for dev in host.input_devices()? {
        let label = dev.id().map(|id| format!("{}", id)).unwrap_or_else(|_| "?".into());
        println!("  {}", label);
    }
    Ok(())
}

fn serve(
    tap_name: &str,
    preset: &str,
    node_id: Option<u8>,
    tx_device: Option<&str>,
    rx_device: Option<&str>,
    difs: u64,
    slot: u64,
    cw_min: u8,
    cw_max: u8,
    max_retries: u8,
    ack_timeout: u64,
) -> Result<()> {
    // Derive a stable default node ID from the hostname so different
    // machines on the same TAP name still get distinct IDs.
    let id = node_id.unwrap_or_else(|| {
        let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
            .unwrap_or_default()
            .trim()
            .to_string();
        let hash = hostname.bytes().fold(0u8, |acc, b| acc.wrapping_add(b));
        if hash == 0 { 1 } else { hash }
    });

    let tap: Box<dyn Tappable> = Box::new(TapInterface::create(Some(tap_name))?);
    log::info!("TAP interface: {}", tap.name_string());
    log::info!("Node ID: {}", id);

    let cfg = Config::from_preset_name(preset);
    let phy = Phy::new(cfg.clone(), tx_device, rx_device)?;
    log::info!("PHY initialized (48 kHz, {} Hz symbol rate)", cfg.symbol_rate() as u32);

    let mac_config = MacConfig {
        node_id: id,
        difs_ms: difs,
        slot_ms: slot,
        cw_min,
        cw_max,
        max_retries,
        ack_timeout_ms: ack_timeout,
    };

    let mut mac = Mac::new(mac_config, phy, tap);
    mac.run()
}
