"""Audio device enumeration and selection utilities."""

from typing import List, Tuple, Optional

import sounddevice as sd


class AudioDeviceInfo:
    """Container for audio device information."""

    def __init__(self, device_dict: dict, kind: str):
        self.index = device_dict.get("index")
        self.name = device_dict.get("name", "Unknown")
        self.host_api = device_dict.get("hostapi", 0)
        self.max_input_channels = device_dict.get("max_input_channels", 0)
        self.max_output_channels = device_dict.get("max_output_channels", 0)
        self.default_sample_rate = device_dict.get("default_samplerate", 48000)
        self.kind = kind  # "input" or "output"

    def __repr__(self) -> str:
        channels = []
        if self.max_input_channels > 0:
            channels.append(f"in:{self.max_input_channels}")
        if self.max_output_channels > 0:
            channels.append(f"out:{self.max_output_channels}")
        ch_str = "+".join(channels) if channels else "none"
        return (
            f"AudioDeviceInfo(index={self.index}, name={self.name!r}, "
            f"channels={ch_str}, kind={self.kind})"
        )


def list_devices() -> List[AudioDeviceInfo]:
    """List all available audio devices."""
    devices = sd.query_devices()
    result = []
    for dev in devices:
        info = AudioDeviceInfo(dev, "both")
        if dev["max_input_channels"] > 0 and dev["max_output_channels"] > 0:
            info.kind = "duplex"
        elif dev["max_input_channels"] > 0:
            info.kind = "input"
        elif dev["max_output_channels"] > 0:
            info.kind = "output"
        else:
            continue  # skip devices with no I/O
        result.append(info)
    return result


def list_input_devices() -> List[AudioDeviceInfo]:
    """List all available input (mic) devices."""
    all_devices = list_devices()
    return [d for d in all_devices if d.max_input_channels > 0]


def list_output_devices() -> List[AudioDeviceInfo]:
    """List all available output (speaker) devices."""
    all_devices = list_devices()
    return [d for d in all_devices if d.max_output_channels > 0]


def list_duplex_devices() -> List[AudioDeviceInfo]:
    """List devices that support both input and output."""
    all_devices = list_devices()
    return [d for d in all_devices if d.kind == "duplex"]


def get_default_input_device() -> Optional[AudioDeviceInfo]:
    """Get the default input device."""
    default_index = sd.default.device[0]
    if default_index is not None:
        devices = sd.query_devices()
        if 0 <= default_index < len(devices):
            return AudioDeviceInfo(devices[default_index], "input")
    return None


def get_default_output_device() -> Optional[AudioDeviceInfo]:
    """Get the default output device."""
    default_index = sd.default.device[1]
    if default_index is not None:
        devices = sd.query_devices()
        if 0 <= default_index < len(devices):
            return AudioDeviceInfo(devices[default_index], "output")
    return None


def print_device_list():
    """Print a formatted list of all audio devices (for debugging)."""
    devices = list_devices()
    print(f"\n{'Index':<6} {'Name':<45} {'In':<4} {'Out':<4} {'SR':<8} {'Kind'}")
    print("-" * 80)
    for d in devices:
        print(
            f"{d.index:<6} {d.name:<45} {d.max_input_channels:<4} "
            f"{d.max_output_channels:<4} {d.default_sample_rate:<8} {d.kind}"
        )
    print()
