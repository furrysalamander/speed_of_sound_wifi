// Local acoustic loopback test using a mock TAP.
//
// This test runs sosw-tap's full MAC + real PHY against a mocked TAP device,
// feeding an Ethernet frame in and expecting to see the same frame emerge
// after acoustic speaker->mic loopback.
//
// It makes noise, so it is marked #[ignore]. Run with:
//   cargo test -p sosw-tap --test mock_tap_loopback -- --ignored --nocapture

use cpal::traits::DeviceTrait;
use sosw_core::Config;
use sosw_tap::mac::{Mac, MacConfig};
use sosw_tap::phy::Phy;
use sosw_tap::tap::MockTap;
use std::time::{Duration, Instant};

fn find_device(name_substring: &str, output: bool) -> Option<cpal::Device> {
    use cpal::traits::HostTrait;
    let host = cpal::default_host();
    let lower = name_substring.to_lowercase();
    let mut devices: Box<dyn Iterator<Item = cpal::Device>> = if output {
        Box::new(host.output_devices().ok()?)
    } else {
        Box::new(host.input_devices().ok()?)
    };
    devices.find(|d| {
        d.id()
            .map(|id| format!("{}", id).to_lowercase().contains(&lower))
            .unwrap_or(false)
    })
}

#[test]
#[ignore]
fn test_mock_tap_acoustic_loopback() {
    let cfg = Config::ofdm_default();
    let tx_dev = find_device("analog", true).expect("no analog output device");
    let rx_dev = find_device("PnP", false).expect("no PnP input device");

    let tap_for_mac = MockTap::new("mock0");
    let tap_for_test = tap_for_mac.clone();

    // Build a dummy Ethernet frame: dst(6) + src(6) + ethertype(2) + payload.
    let mut eth_frame = Vec::new();
    eth_frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x01]); // dst
    eth_frame.extend_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]); // src
    eth_frame.extend_from_slice(&[0x08, 0x00]); // IPv4 ethertype
    eth_frame.extend_from_slice(b"hello from mock tap loopback");

    // Inject the frame into the mock TAP so the MAC will pick it up and TX it.
    tap_for_mac.inject(eth_frame.clone());

    let tx_id = format!("{}", tx_dev.id().unwrap());
    let rx_id = format!("{}", rx_dev.id().unwrap());
    let phy = Phy::new(cfg.clone(), Some(&tx_id), Some(&rx_id))
        .expect("failed to create PHY");

    let mac_config = MacConfig {
        node_id: 79,
        difs_ms: 600,
        slot_ms: 60,
        cw_min: 4,
        cw_max: 64,
        max_retries: 5,
        ack_timeout_ms: 2000,
    };

    let mut mac = Mac::new(mac_config, phy, Box::new(tap_for_mac));

    // Drive the MAC manually for up to ~15 seconds. The MAC will:
    // 1. Read the Ethernet frame from mock TAP
    // 2. Fragment and transmit it over the acoustic channel
    // 3. Receive the loopback audio via microphone
    // 4. Reassemble and send the frame back out through mock TAP
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        mac.run_one_iteration().expect("MAC iteration failed");
        std::thread::sleep(Duration::from_millis(20));
    }

    // The reassembled frame should have been sent back through the mock TAP.
    let sent = tap_for_test.sent();
    assert!(
        sent.is_some(),
        "expected reassembled Ethernet frame to be sent back through mock TAP"
    );
    let sent_frame = sent.unwrap();
    assert_eq!(
        sent_frame, eth_frame,
        "reassembled frame does not match injected frame"
    );
    println!("Acoustic loopback through mock TAP succeeded");
}
