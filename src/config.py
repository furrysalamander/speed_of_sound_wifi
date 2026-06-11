"""Configuration management for the audio data transmission system."""

from dataclasses import dataclass, field
from typing import Optional, Tuple


@dataclass
class AudioConfig:
    """Audio I/O configuration."""

    sample_rate: int = 48000
    buffer_size: int = 256  # samples per callback
    channels: int = 1  # mono
    device_input_index: Optional[int] = None
    device_output_index: Optional[int] = None
    device_input_name: Optional[str] = None
    device_output_name: Optional[str] = None


@dataclass
class ModulationConfig:
    """Physical layer modulation configuration."""

    baud_rate: int = 1000  # symbols per second (FSK only)
    m_fsk: int = 4  # number of FSK tones (2, 4, 8, 16)
    freq_min: int = 200  # minimum frequency in Hz
    freq_max: int = 18000  # maximum frequency in Hz
    window_type: str = "hann"  # window function for tone generation
    use_ofdm: bool = False  # use OFDM instead of FSK
    output_amplitude: float = 0.08  # peak output amplitude. USB mic: 0.05-0.10 avoids AGC clipping


@dataclass
class OfdmConfig:
    """OFDM modulation configuration."""

    fft_size: int = 256
    cp_length: int = 32  # shorter CP for higher throughput (167 Hz sym rate vs 125)
    subcarrier_min: int = 9  # first active subcarrier (1687 Hz, past 1500 Hz dip)
    subcarrier_max: int = 43  # last active subcarrier (8062 Hz, before rolloff)
    bits_per_subcarrier: int = 2  # BPSK=1, QPSK=2
    pilot_subcarriers: tuple = ()  # no pilots; DD tracking sufficient with scrambler
    preamble_symbols: int = 4  # number of OFDM symbols in preamble (more = better channel estimate)


@dataclass
class FecConfig:
    """Forward Error Correction configuration."""

    enabled: bool = True
    nsym: int = 32  # number of parity symbols for Reed-Solomon
    # RS(255, 223) by default: 223 data bytes + 32 parity bytes


@dataclass
class FrameConfig:
    """Frame protocol configuration."""

    payload_size: int = 442  # bytes per frame payload (2 RS blocks: 446/223 = exact fit)
    sync_pattern: bytes = field(
        default_factory=lambda: b"\xAA\x55\xAA\x55\xAA\x55\xAA\x55"
    )
    max_retransmissions: int = 5


@dataclass
class ProtocolConfig:
    """File transfer protocol configuration."""

    chunk_size: int = 1024  # bytes to read per chunk from file
    ack_timeout: float = 5.0  # seconds to wait for ACK
    progress_interval: float = 0.1  # seconds between progress updates


@dataclass
class UiConfig:
    """UI configuration."""

    waterfall_fft_size: int = 1024  # FFT size for waterfall display
    waterfall_update_rate: int = 30  # target FPS for waterfall updates
    waterfall_cols: int = 256  # number of columns in waterfall display
    title: str = "Speed of Sound WiFi"


@dataclass
class Config:
    """Master configuration."""

    audio: AudioConfig = field(default_factory=AudioConfig)
    modulation: ModulationConfig = field(default_factory=ModulationConfig)
    ofdm: OfdmConfig = field(default_factory=OfdmConfig)
    fec: FecConfig = field(default_factory=FecConfig)
    frame: FrameConfig = field(default_factory=FrameConfig)
    protocol: ProtocolConfig = field(default_factory=ProtocolConfig)
    ui: UiConfig = field(default_factory=UiConfig)

    @property
    def bits_per_symbol(self) -> int:
        """Calculate bits per symbol from M-FSK setting."""
        import math

        return int(math.log2(self.modulation.m_fsk))

    @property
    def theoretical_bps(self) -> int:
        """Calculate theoretical bits per second."""
        return self.modulation.baud_rate * self.bits_per_symbol

    @property
    def symbol_duration_samples(self) -> int:
        """Calculate symbol duration in samples."""
        return self.audio.sample_rate // self.modulation.baud_rate

    @property
    def freq_spacing(self) -> float:
        """Calculate frequency spacing between FSK tones."""
        return (self.modulation.freq_max - self.modulation.freq_min) / self.modulation.m_fsk

    @property
    def fsk_frequencies(self) -> list:
        """Calculate the actual frequencies for each FSK tone."""
        freqs = []
        for i in range(self.modulation.m_fsk):
            freq = self.modulation.freq_min + (i + 0.5) * self.freq_spacing
            freqs.append(freq)
        return freqs

    @property
    def ofdm_subcarrier_count(self) -> int:
        """Number of active OFDM subcarriers."""
        return self.ofdm.subcarrier_max - self.ofdm.subcarrier_min + 1

    @property
    def ofdm_symbol_samples(self) -> int:
        """Total samples per OFDM symbol (CP + FFT)."""
        return self.ofdm.fft_size + self.ofdm.cp_length

    @property
    def ofdm_symbol_rate(self) -> float:
        """OFDM symbol rate in Hz."""
        return self.audio.sample_rate / self.ofdm_symbol_samples

    @property
    def ofdm_bits_per_symbol(self) -> int:
        """Total raw bits per OFDM symbol."""
        return self.ofdm_subcarrier_count * self.ofdm.bits_per_subcarrier

    @property
    def theoretical_bps(self) -> int:
        """Calculate theoretical bits per second."""
        if self.modulation.use_ofdm:
            return int(self.ofdm_symbol_rate * self.ofdm_bits_per_symbol)
        return self.modulation.baud_rate * self.bits_per_symbol

    def to_dict(self) -> dict:
        """Convert to dictionary for serialization."""
        import dataclasses

        return dataclasses.asdict(self)
