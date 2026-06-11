"""Post-process a saved OTA recording to determine why frames were dropped.

Usage:
    python -m examples.analyze_ota_rca <save_dir> <payload_file>
"""

import json
import os
import sys

import numpy as np

from src.config import Config


def main():
    if len(sys.argv) < 3:
        print("Usage: python -m examples.analyze_ota_rca "
              "<save_dir> <payload_file>")
        return 1

    save_dir = sys.argv[1]
    payload_file = sys.argv[2]

    config = Config()
    config.modulation.use_ofdm = True

    from src.physical.ofdm import OfdmModulator, OfdmDemodulator
    mod = OfdmModulator(config)
    dem = OfdmDemodulator(config)
    preamble = dem._preamble_audio
    preamble_energy = float(np.dot(preamble, preamble))

    sym_len = dem.sym_samples
    plen = dem.preamble_symbols * sym_len
    from examples.demo_rx import frame_data_syms
    ds = frame_data_syms(config)
    frame_samples = plen + ds * sym_len

    recording = np.load(os.path.join(save_dir, "full_recording.npy"))
    print(f"Recording: {len(recording)} samples = {len(recording)/48000:.1f}s")

    with open(payload_file, "rb") as f:
        payload = f.read()
    ps = config.frame.payload_size
    n_frames = len(payload) // ps

    # Dense correlation across the full recording
    xcorr = np.convolve(recording, preamble[::-1], mode="valid")
    xcorr_abs = np.abs(xcorr)

    # Find the first major peak (preamble of frame 0)
    # Slide a frame_samples window looking for the strongest peak
    window_energy = np.convolve(recording ** 2,
                                np.ones(frame_samples), mode="valid")
    # Find region with highest energy
    energy_peak = int(np.argmax(window_energy))
    # Within that region, find the strongest preamble correlation
    search_start = max(0, energy_peak - frame_samples)
    search_end = min(len(xcorr_abs), energy_peak + frame_samples)
    # But to be more robust, scan: advance by frame_samples, find local peaks
    # Strategy: step through the recording in frame_samples increments
    # starting from the max correlation point

    # Global max of correlation
    global_max_idx = int(np.argmax(xcorr_abs))
    global_max_val = float(xcorr_abs[global_max_idx])
    global_npk = global_max_val / np.sqrt(
        preamble_energy * float(np.dot(
            recording[global_max_idx:global_max_idx + plen],
            recording[global_max_idx:global_max_idx + plen])) + 1e-10
    )

    print(f"\nGlobal preamble correlation peak:")
    print(f"  Position: {global_max_idx} samples ({global_max_idx/48000:.2f}s)")
    print(f"  Value: {global_max_val:.4f}")
    print(f"  Norm peak: {global_npk:.4f}")

    # Now find peaks at expected frame positions
    # Use the global peak as frame 0 anchor, then step forward
    anchor = global_max_idx

    print(f"\n{'Frame':>6} {'ExpPos':>8} {'PeakPos':>8} {'Delta':>6} "
          f"{'PkVal':>8} {'NPK':>6} {'SNR':>6} {'Status'}")
    print("-" * 70)

    found_count = 0
    lost_frames = []

    for i in range(n_frames):
        expected_pos = anchor + i * frame_samples
        if expected_pos + plen >= len(recording):
            break

        # Measure npk at the exact expected position
        seg = recording[expected_pos:expected_pos + plen]
        se = float(np.dot(seg, seg))
        npk_exact = (float(np.abs(xcorr[expected_pos])) /
                     (np.sqrt(preamble_energy * se) + 1e-10))

        # Find the best peak within ±preamble_samples around expected
        half = int(plen * 0.5)
        left = max(0, expected_pos - half)
        right = min(len(xcorr_abs), expected_pos + half)
        local_region = xcorr_abs[left:right]
        if len(local_region) == 0:
            continue
        local_peak_idx = int(np.argmax(local_region)) + left
        local_peak_val = float(xcorr_abs[local_peak_idx])
        delta = local_peak_idx - expected_pos
        seg2 = recording[local_peak_idx:local_peak_idx + plen]
        se2 = float(np.dot(seg2, seg2))
        npk_local = (local_peak_val /
                     (np.sqrt(preamble_energy * se2) + 1e-10))

        # Noise estimate: average correlation in adjacent region (±2 frames)
        noise_region = np.concatenate([
            xcorr_abs[max(0, expected_pos - 2 * frame_samples):
                      max(0, expected_pos - frame_samples)],
            xcorr_abs[min(len(xcorr_abs), expected_pos + frame_samples):
                      min(len(xcorr_abs), expected_pos + 2 * frame_samples)],
        ]) if expected_pos > 2 * frame_samples else xcorr_abs[:frame_samples]
        noise_rms = float(np.sqrt(np.mean(noise_region ** 2))) if len(noise_region) > 0 else 0
        snr = local_peak_val / (noise_rms + 1e-10)

        status = ""
        if npk_local >= 0.10:
            status = "OK"
            found_count += 1
        elif npk_local >= 0.05:
            status = "LOW"
            lost_frames.append((i, npk_local, snr))
        else:
            status = "MISS"
            lost_frames.append((i, npk_local, snr))

        print(f"{i:>6} {expected_pos:>8} {local_peak_idx:>8} "
              f"{delta:>+6} {local_peak_val:>8.2f} "
              f"{npk_local:>6.3f} {snr:>6.1f} {status}")

    print(f"\nSummary: {found_count}/{n_frames} frames above npk=0.10 threshold")
    if lost_frames:
        print(f"\nLost/dim frames ({len(lost_frames)}):")
        for i, npk, snr in lost_frames:
            expected_pos = anchor + i * frame_samples
            left = max(0, expected_pos - plen)
            right = min(len(recording), expected_pos + 2 * plen)
            seg_energy = float(np.dot(
                recording[left:right], recording[left:right]))
            seg_dur = (right - left) / 48000
            print(f"  Frame {i}: npk={npk:.4f}, snr={snr:.1f}, "
                  f"seg_energy={seg_energy:.2f} over {seg_dur*1000:.0f}ms")

    return 0


if __name__ == "__main__":
    sys.exit(main())
