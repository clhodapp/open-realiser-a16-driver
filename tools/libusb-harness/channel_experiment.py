#!/usr/bin/env python3
"""Quiet, file-based A16 channel probes and narrowband capture measurements.

No USB access: generate WAVs, run ozzy-libusb separately, then analyze its WAVs.
All levels are digital full scale, not a prediction of headphone sound pressure.
"""
import argparse
import cmath
import json
import math
from pathlib import Path
import wave

RATE = 44100
FREQUENCIES = [300 + 40 * channel for channel in range(16)]
AMPLITUDE = 0.001  # -60 dBFS peak, per active playback channel


def generate(directory):
    directory.mkdir(parents=True, exist_ok=True)
    plans = {
        "sequential": {"seconds": 18, "segments": [
            {"start": 1 + channel, "duration": 0.6, "channels": [channel + 1]}
            for channel in range(16)]},
        "simultaneous": {"seconds": 12, "segments": [
            {"start": 1, "duration": 2, "channels": [1, 2]},
            {"start": 4, "duration": 2, "channels": list(range(1, 9))},
            {"start": 7, "duration": 2, "channels": list(range(1, 17))}]},
    }
    for name, plan in plans.items():
        data = bytearray(plan["seconds"] * RATE * 48)
        for segment in plan["segments"]:
            first = round(segment["start"] * RATE)
            length = round(segment["duration"] * RATE)
            for channel in segment["channels"]:
                frequency = FREQUENCIES[channel - 1]
                for sample in range(length):
                    fade = min(sample / 441, (length - 1 - sample) / 441, 1)
                    gain = 0.5 - 0.5 * math.cos(math.pi * max(0, fade))
                    value = round(8388607 * AMPLITUDE * gain * math.sin(2 * math.pi * frequency * sample / RATE))
                    at = (first + sample) * 48 + (channel - 1) * 3
                    data[at:at + 3] = value.to_bytes(3, "little", signed=True)
        with (directory / f"{name}-input.wav").open("xb") as file:
            with wave.open(file, "wb") as wav:
                wav.setnchannels(16)
                wav.setsampwidth(3)
                wav.setframerate(RATE)
                wav.writeframes(data)
    with (directory / "plan.json").open("x") as file:
        json.dump({"rate": RATE, "peak_dbfs_per_channel": -60,
                   "frequencies": FREQUENCIES, "experiments": plans}, file, indent=2)
        file.write("\n")


def read_return(path):
    with wave.open(str(path), "rb") as wav:
        if (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) != (RATE, 2, 3):
            raise ValueError("expected 44100 Hz stereo packed 24-bit capture")
        data = wav.readframes(wav.getnframes())
    return [[int.from_bytes(data[i:i + 3], "little", signed=True)
             for i in range(channel * 3, len(data), 6)] for channel in range(2)]


def dbfs(value):
    return 20 * math.log10(value / 8388608) if value else None


def phasor(samples, frequency):
    # Use absolute capture-frame indices to make phases comparable within a run.
    # Central 0.2 s is an integer number of cycles for every probe frequency.
    first, values = samples
    step = cmath.exp(-2j * math.pi * frequency / RATE)
    carrier = step ** first
    total = 0j
    for value in values:
        total += value * carrier
        carrier *= step
    return 2 * total / len(values)


def analyze(directory):
    plan = json.loads((directory / "plan.json").read_text())
    result = {}
    for name, experiment in plan["experiments"].items():
        capture = read_return(directory / f"{name}-return.wav")
        measurements = []
        for segment in experiment["segments"]:
            center = segment["start"] + segment["duration"] / 2
            first, end = round((center - 0.1) * RATE), round((center + 0.1) * RATE)
            if end > len(capture[0]):
                raise ValueError(f"{name}: capture ends before analysis window")
            entry = {"start_s": segment["start"], "active_channels": segment["channels"], "return": []}
            for values in capture:
                window = values[first:end]
                tones = [phasor((first, window), frequency) for frequency in FREQUENCIES]
                entry["return"].append({
                    "rms_dbfs": dbfs(math.sqrt(sum(value * value for value in window) / len(window))),
                    "tone_peak_dbfs": [dbfs(abs(value)) for value in tones],
                    "tone_phase_rad": [cmath.phase(value) for value in tones],
                })
            measurements.append(entry)
        result[name] = {"frames": len(capture[0]), "measurements": measurements}
    single = result["sequential"]["measurements"]
    for group in result["simultaneous"]["measurements"]:
        group["gain_change_db_vs_single"] = [
            [group["return"][side]["tone_peak_dbfs"][channel - 1] -
             single[channel - 1]["return"][side]["tone_peak_dbfs"][channel - 1]
             if group["return"][side]["tone_peak_dbfs"][channel - 1] is not None and
             single[channel - 1]["return"][side]["tone_peak_dbfs"][channel - 1] is not None and
             single[channel - 1]["return"][side]["tone_peak_dbfs"][channel - 1] > -140 else None
             for channel in group["active_channels"]] for side in range(2)]
    with (directory / "analysis.json").open("w") as file:
        json.dump(result, file, indent=2)
        file.write("\n")
    print("Channel  Frequency  Return-L dBFS  Return-R dBFS")
    for i, entry in enumerate(single):
        levels = [entry["return"][side]["tone_peak_dbfs"][i] for side in range(2)]
        print(f"{i+1:7}  {FREQUENCIES[i]:9}  " + "  ".join(f"{level:13.2f}" if level is not None else "         -inf" for level in levels))
    for group in result["simultaneous"]["measurements"]:
        print("Simultaneous channels", group["active_channels"])
        for side, changes in zip(["L", "R"], group["gain_change_db_vs_single"]):
            print(side, "gain changes dB:", [round(value, 2) if value is not None else None for value in changes])
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["generate", "analyze"])
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    (generate if args.command == "generate" else analyze)(args.directory)
