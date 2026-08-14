#!/usr/bin/env python3
"""Decode Polar Stream's experimental stereo PCM data-link recording.

The browser/native UI emits a cable-oriented Manchester waveform. This tool
accepts an uncompressed stereo PCM WAV captured from a line input or digital
loopback and reconstructs CRC-valid ECG, accelerometer, and metric rows.
"""

from __future__ import annotations

import argparse
import csv
from dataclasses import dataclass
from pathlib import Path
import struct
import sys
import wave
import zlib


SYMBOL_RATE = 11_025
PREAMBLE = bytes([0x55] * 8 + [0xD3, 0x91])
MAGIC = b"PS"
SECTION_ECG = 1
SECTION_ACC = 2
SECTION_METRICS = 3
UNITS = {
    "raw_ecg": "uV",
    "raw_acc": "mg",
    "heart_rate": "bpm",
    "rr_interval": "ms",
    "acc_magnitude": "g",
    "acc_breathing_magnitude": "g",
    "breathing_phase": "class",
}


@dataclass(frozen=True)
class AudioRecording:
    sample_rate: int
    left: list[float]
    right: list[float]


@dataclass(frozen=True)
class DecodedPacket:
    sequence: int
    schema_version: int
    payload: bytes


def pcm_sample(data: bytes, offset: int, width: int) -> float:
    if width == 1:
        return (data[offset] - 128) / 128
    if width == 2:
        return int.from_bytes(data[offset : offset + 2], "little", signed=True) / 32_768
    if width == 3:
        raw = int.from_bytes(data[offset : offset + 3], "little", signed=False)
        if raw & 0x80_0000:
            raw -= 0x100_0000
        return raw / 8_388_608
    if width == 4:
        return int.from_bytes(data[offset : offset + 4], "little", signed=True) / 2_147_483_648
    raise ValueError(f"Unsupported PCM sample width: {width} bytes")


def read_wav(path: Path) -> AudioRecording:
    with wave.open(str(path), "rb") as source:
        if source.getcomptype() != "NONE":
            raise ValueError("The decoder requires an uncompressed PCM WAV file.")
        channels = source.getnchannels()
        if channels < 2:
            raise ValueError("The audio data link requires a stereo recording.")
        sample_rate = source.getframerate()
        width = source.getsampwidth()
        frames = source.readframes(source.getnframes())
    frame_width = channels * width
    left: list[float] = []
    right: list[float] = []
    for offset in range(0, len(frames) - frame_width + 1, frame_width):
        left.append(pcm_sample(frames, offset, width))
        right.append(pcm_sample(frames, offset + width, width))
    return AudioRecording(sample_rate=sample_rate, left=left, right=right)


def find_bursts(recording: AudioRecording, threshold: float) -> list[tuple[int, int]]:
    minimum_silence = max(8, round(recording.sample_rate * 0.0015))
    bursts: list[tuple[int, int]] = []
    start: int | None = None
    silent = minimum_silence
    for index, (left, right) in enumerate(zip(recording.left, recording.right, strict=True)):
        active = max(abs(left), abs(right)) >= threshold
        if active:
            if start is None and silent >= minimum_silence:
                start = index
            silent = 0
        else:
            silent += 1
            if start is not None and silent >= minimum_silence:
                end = index - silent + 1
                if end > start:
                    bursts.append((start, end))
                start = None
    if start is not None:
        bursts.append((start, len(recording.left)))
    return bursts


def bits_to_bytes(bits: list[int], offset: int) -> bytes:
    count = (len(bits) - offset) // 8
    result = bytearray(count)
    for byte_index in range(count):
        value = 0
        for bit in bits[offset + byte_index * 8 : offset + byte_index * 8 + 8]:
            value = (value << 1) | bit
        result[byte_index] = value
    return bytes(result)


def sample_nearest(values: list[float], position: float) -> float:
    index = min(len(values) - 1, max(0, round(position)))
    return values[index]


def decode_symbols(
    recording: AudioRecording,
    start: int,
    end: int,
    timing_shift: float,
    swap_channels: bool,
    invert: bool,
) -> list[int]:
    left, right = (recording.right, recording.left) if swap_channels else (recording.left, recording.right)
    samples_per_symbol = recording.sample_rate / SYMBOL_RATE
    symbol_count = max(0, int((end - start - abs(timing_shift)) / samples_per_symbol))
    bits: list[int] = []
    for symbol in range(symbol_count):
        base = start + timing_shift + symbol * samples_per_symbol
        first = base + samples_per_symbol * 0.25
        second = base + samples_per_symbol * 0.75
        left_bit = int(sample_nearest(left, first) > sample_nearest(left, second))
        right_bit = int(sample_nearest(right, first) > sample_nearest(right, second))
        if invert:
            left_bit ^= 1
            right_bit ^= 1
        bits.extend((left_bit, right_bit))
    return bits


def parse_packet(packet: bytes) -> DecodedPacket | None:
    if len(packet) < 11 or packet[:2] != MAGIC:
        return None
    payload_length = int.from_bytes(packet[5:7], "little")
    expected_length = 7 + payload_length + 4
    if len(packet) < expected_length:
        return None
    candidate = packet[:expected_length]
    expected_crc = int.from_bytes(candidate[-4:], "little")
    actual_crc = zlib.crc32(candidate[:-4]) & 0xFFFF_FFFF
    if expected_crc != actual_crc:
        return None
    return DecodedPacket(
        sequence=int.from_bytes(candidate[3:5], "little"),
        schema_version=candidate[2] >> 4,
        payload=candidate[7:-4],
    )


def decode_burst(recording: AudioRecording, start: int, end: int) -> DecodedPacket | None:
    # A line-level recording normally aligns within one sample. Trying small
    # fractional offsets, polarity inversion, and channel swap makes decoding
    # tolerant of common capture-device transformations without guessing data.
    timing_shifts = [step / 8 for step in range(-12, 13)]
    for swap_channels in (False, True):
        for invert in (False, True):
            for timing_shift in timing_shifts:
                bits = decode_symbols(
                    recording,
                    start,
                    end,
                    timing_shift,
                    swap_channels,
                    invert,
                )
                for bit_offset in range(8):
                    decoded = bits_to_bytes(bits, bit_offset)
                    cursor = decoded.find(PREAMBLE)
                    while cursor >= 0:
                        packet = parse_packet(decoded[cursor + len(PREAMBLE) :])
                        if packet is not None:
                            return packet
                        cursor = decoded.find(PREAMBLE, cursor + 1)
    return None


def signed_int24(payload: bytes, offset: int) -> int:
    value = int.from_bytes(payload[offset : offset + 3], "little")
    return value - 0x100_0000 if value & 0x80_0000 else value


def sample_timestamp(final_timestamp_ns: int, index: int, count: int, rate_hz: int) -> str:
    if final_timestamp_ns == 0:
        return ""
    offset = round((count - 1 - index) * 1_000_000_000 / rate_hz)
    return str(max(0, final_timestamp_ns - offset))


def packet_rows(packet: DecodedPacket) -> list[list[object]]:
    payload = packet.payload
    if not payload:
        return []
    section_count = payload[0]
    cursor = 1
    rows: list[list[object]] = []
    for _ in range(section_count):
        if cursor + 13 > len(payload):
            raise ValueError(f"Packet {packet.sequence} has a truncated section header.")
        kind = payload[cursor]
        count = int.from_bytes(payload[cursor + 1 : cursor + 3], "little")
        timestamp = int.from_bytes(payload[cursor + 3 : cursor + 11], "little")
        length = int.from_bytes(payload[cursor + 11 : cursor + 13], "little")
        cursor += 13
        section = payload[cursor : cursor + length]
        if len(section) != length:
            raise ValueError(f"Packet {packet.sequence} has a truncated section payload.")
        cursor += length
        if kind == SECTION_ECG:
            if length != count * 3:
                raise ValueError(f"Packet {packet.sequence} has an invalid ECG section.")
            for index in range(count):
                value = signed_int24(section, index * 3)
                rows.append([
                    packet.sequence,
                    sample_timestamp(timestamp, index, count, 130),
                    "raw_ecg",
                    index,
                    "",
                    "",
                    "",
                    value,
                    "uV",
                ])
        elif kind == SECTION_ACC:
            if length != count * 6:
                raise ValueError(f"Packet {packet.sequence} has an invalid accelerometer section.")
            for index in range(count):
                x, y, z = struct.unpack_from("<hhh", section, index * 6)
                rows.append([
                    packet.sequence,
                    sample_timestamp(timestamp, index, count, 200),
                    "raw_acc",
                    index,
                    x,
                    y,
                    z,
                    "",
                    "mg",
                ])
        elif kind == SECTION_METRICS:
            metric_cursor = 0
            for index in range(count):
                if metric_cursor >= len(section):
                    raise ValueError(f"Packet {packet.sequence} has a truncated metric section.")
                id_length = section[metric_cursor]
                metric_cursor += 1
                end = metric_cursor + id_length
                if end + 4 > len(section):
                    raise ValueError(f"Packet {packet.sequence} has a truncated metric value.")
                metric_id = section[metric_cursor:end].decode("utf-8")
                value = struct.unpack_from("<f", section, end)[0]
                metric_cursor = end + 4
                rows.append([
                    packet.sequence,
                    "",
                    metric_id,
                    index,
                    "",
                    "",
                    "",
                    value,
                    UNITS.get(metric_id, ""),
                ])
    return rows


def write_csv(path: Path, packets: list[DecodedPacket], source: Path) -> int:
    rows = 0
    with path.open("w", encoding="utf-8", newline="") as target:
        target.write("# Polar Stream decoded audio data\n")
        target.write("# audio_schema_version,1\n")
        target.write(f"# source_wav,{source.name}\n")
        writer = csv.writer(target)
        writer.writerow([
            "packet_sequence",
            "sensor_timestamp_ns",
            "stream",
            "sample_index",
            "x_mg",
            "y_mg",
            "z_mg",
            "value",
            "unit",
        ])
        for packet in packets:
            packet_data = packet_rows(packet)
            writer.writerows(packet_data)
            rows += len(packet_data)
    return rows


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="stereo uncompressed PCM WAV recording")
    parser.add_argument("--output", type=Path, help="decoded CSV path")
    parser.add_argument(
        "--threshold",
        type=float,
        default=0.015,
        help="normalized line-level burst threshold (default: 0.015)",
    )
    arguments = parser.parse_args()
    if not 0 < arguments.threshold < 1:
        parser.error("--threshold must be between 0 and 1")
    output = arguments.output or arguments.input.with_suffix(".decoded.csv")
    try:
        recording = read_wav(arguments.input)
        bursts = find_bursts(recording, arguments.threshold)
        packets = [
            packet
            for start, end in bursts
            if (packet := decode_burst(recording, start, end)) is not None
        ]
        if not packets:
            raise ValueError(
                "No CRC-valid Polar Stream frames were found. Check that the recording is stereo PCM, "
                "that audio enhancements/noise suppression are off, and adjust --threshold if needed."
            )
        rows = write_csv(output, packets, arguments.input)
    except (OSError, EOFError, ValueError, wave.Error) as error:
        print(f"Decode failed: {error}", file=sys.stderr)
        return 2
    print(f"Decoded {len(packets)} CRC-valid frames and {rows} data rows into {output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
