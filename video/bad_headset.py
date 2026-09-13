"""Generate ElevenLabs speech and degrade it to sound like a cheap call-centre headset.

Usage:
    python bad_headset.py "hello this is albert from microsoft tech support"
    python bad_headset.py "text" -o albert_intro
    python bad_headset.py "text" -o albert_intro --speed 1.4
    python bad_headset.py --degrade-only audio/some_raw_1.2x.wav   # re-process without an API call

Outputs go to ./audio/:
    <name>_raw_1.2x.wav                        untouched ElevenLabs output
    <name>_<speed>x_clean.wav                  sped up, no effects
    <name>_<speed>x_grit<g>_noise<n>_bad.wav   sped up, bad headset
"""
import argparse
import json
import os
import re
import sys
import urllib.error
import urllib.request
import wave
from pathlib import Path

import numpy as np
from scipy.signal import butter, resample_poly, sosfilt

HERE = Path(__file__).resolve().parent
AUDIO_DIR = HERE / "audio"

VOICE_ID = "xnx6sPTtvU635ocDt2j7"
MODEL_ID = "eleven_multilingual_v2"
API_MAX_SPEED = 1.2  # ElevenLabs rejects anything higher
SAMPLE_RATE = 24000  # raw PCM, no decoder needed
DEFAULT_SPEED = 1.3
DEFAULT_NOISE = 0.3  # scales hiss and background murmur (1.0 = loud, 0 = none)
DEFAULT_GRIT = 0.3  # distortion/codec crunch on the voice (0 = clean, 1 = very harsh)


def load_api_key():
    key = os.environ.get("ELEVENLABS_API_KEY")
    env_file = HERE / ".env"
    if not key and env_file.exists():
        for line in env_file.read_text().splitlines():
            if line.startswith("ELEVENLABS_API_KEY="):
                key = line.split("=", 1)[1].strip()
    if not key:
        sys.exit("No ELEVENLABS_API_KEY in environment or .env")
    return key


def tts(text):
    url = (f"https://api.elevenlabs.io/v1/text-to-speech/{VOICE_ID}"
           f"?output_format=pcm_{SAMPLE_RATE}")
    body = {
        "text": text,
        "model_id": MODEL_ID,
        "voice_settings": {
            "stability": 0.4,
            "similarity_boost": 0.8,
            "speed": API_MAX_SPEED,
        },
    }
    req = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST", headers={
        "xi-api-key": load_api_key(),
        "Content-Type": "application/json",
        "Accept": "audio/pcm",
    })
    try:
        with urllib.request.urlopen(req) as resp:
            pcm = resp.read()
    except urllib.error.HTTPError as e:
        sys.exit(f"ElevenLabs error {e.code}: {e.read().decode(errors='replace')}")
    return np.frombuffer(pcm, dtype="<i2").astype(np.float32) / 32768.0


def read_wav(path):
    with wave.open(str(path), "rb") as w:
        sr, ch, width = w.getframerate(), w.getnchannels(), w.getsampwidth()
        raw = w.readframes(w.getnframes())
    if width != 2:
        sys.exit("Only 16-bit WAV supported")
    x = np.frombuffer(raw, dtype="<i2").astype(np.float32) / 32768.0
    return x.reshape(-1, ch).mean(axis=1), sr


def write_wav(path, x, sr):
    x = np.clip(x, -1, 1)
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(sr)
        w.writeframes((x * 32767).astype("<i2").tobytes())


def time_stretch(x, rate, sr):
    """Speed up by `rate` without changing pitch (WSOLA overlap-add)."""
    if abs(rate - 1) < 1e-3:
        return x
    frame = int(0.030 * sr)
    hop = frame // 2
    tol = int(0.008 * sr)
    win = np.hanning(frame).astype(np.float32)
    n_out = int(len(x) / rate)
    x = np.concatenate([x, np.zeros(2 * frame + 2 * tol, dtype=np.float32)])
    out = np.zeros(n_out + frame, dtype=np.float32)
    norm = np.zeros_like(out)
    prev = 0
    for o in range(0, n_out, hop):
        nominal = int(o * rate)
        if o == 0:
            pos = 0
        else:
            # pick the input frame that best continues the previous one
            target = x[prev + hop:prev + hop + frame]
            lo = max(0, nominal - tol)
            seg = x[lo:nominal + tol + frame]
            pos = lo + int(np.argmax(np.correlate(seg, target, mode="valid")))
        out[o:o + frame] += x[pos:pos + frame] * win
        norm[o:o + frame] += win
        prev = pos
    return out[:n_out] / np.maximum(norm[:n_out], 0.1)


def degrade(x, sr, seed=None, noise=DEFAULT_NOISE, grit=DEFAULT_GRIT):
    """Cheap USB headset on a bad VoIP line."""
    rng = np.random.default_rng(seed)
    n = len(x)
    t = np.arange(n) / sr
    x = x / (np.max(np.abs(x)) + 1e-9)

    # Boom mic too close to the mouth: boost low-mids before band-limiting
    sos = butter(2, [150, 900], btype="band", fs=sr, output="sos")
    x = x + 0.6 * sosfilt(sos, x)

    # Narrow telephone band (tinny)
    sos = butter(6, [350, 3200], btype="band", fs=sr, output="sos")
    x = sosfilt(sos, x)

    # Harsh presence peak from a cheap capsule
    sos = butter(2, [1800, 2600], btype="band", fs=sr, output="sos")
    x = x + 0.8 * sosfilt(sos, x)

    # Headset DSP compression: evens out loudness so speech stays loud
    env = np.abs(x)
    sos = butter(1, 30, btype="low", fs=sr, output="sos")
    env = sosfilt(sos, env)
    # floor caps the boost on quiet parts so silence isn't pumped up into static
    x = x / np.power(np.maximum(env / env.max(), 0.1), 0.4)
    x = x / (np.max(np.abs(x)) + 1e-9)

    # Mic gain a bit hot: soft overdrive (grit 0 = none, 1 = very crunchy)
    drive = 1 + 2.5 * grit
    x = np.tanh(x * drive) / np.tanh(drive)

    # Slow level wobble (head moving relative to boom)
    x *= 1 + 0.08 * np.sin(2 * np.pi * 0.7 * t + rng.uniform(0, 6.28))

    # Background: hiss and faint call-centre murmur
    hiss = rng.standard_normal(n)
    sos = butter(2, [2000, 7000], btype="band", fs=sr, output="sos")
    x += noise * 0.02 * sosfilt(sos, hiss)
    babble = rng.standard_normal(n)
    sos = butter(2, [300, 1500], btype="band", fs=sr, output="sos")
    babble = sosfilt(sos, babble) * (0.5 + 0.5 * np.sin(2 * np.pi * 3.1 * t) ** 2)
    x += noise * 0.03 * babble

    # Downsample to 8 kHz and back: VoIP muffling
    x = resample_poly(resample_poly(x, 1, sr // 8000), sr // 8000, 1)[:n]

    # Codec quantisation: 16 bits at grit 0 down to 7 bits at grit 1
    bits = round(16 - 9 * grit)
    if bits < 16:
        levels = 2 ** bits
        x = np.round(x * levels / 2) / (levels / 2)

    # Occasional packet-loss dropouts, more likely with more grit
    for _ in range(max(1, int(n / sr / 2.5))):
        if rng.random() < 0.6 * grit:
            start = rng.integers(0, max(1, n - sr // 10))
            length = int(rng.uniform(0.03, 0.09) * sr)
            x[start:start + length] *= 0.05

    return x / (np.max(np.abs(x)) + 1e-9) * 0.95


def slugify(text):
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")[:40] or "clip"


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("text", nargs="?", help="Text to speak")
    p.add_argument("-o", "--name", help="Output base name")
    p.add_argument("--degrade-only", metavar="WAV", help="Skip TTS, degrade an existing 16-bit WAV")
    p.add_argument("--seed", type=int, default=None, help="Seed for repeatable noise/dropouts")
    p.add_argument("-n", "--noise", type=float, default=DEFAULT_NOISE,
                   help="Background noise level (1.0 = loud, 0 = none)")
    p.add_argument("-g", "--grit", type=float, default=DEFAULT_GRIT,
                   help="Distortion/crunch on the voice (0 = clean, 1 = very harsh)")
    p.add_argument("-s", "--speed", type=float, default=DEFAULT_SPEED,
                   help="Final speed. ElevenLabs generates at 1.2; anything above is time-stretched "
                        "(pitch kept). --degrade-only assumes the WAV is the raw 1.2 output.")
    args = p.parse_args()

    AUDIO_DIR.mkdir(exist_ok=True)

    if args.degrade_only:
        src = Path(args.degrade_only)
        x, sr = read_wav(src)
        name = args.name or src.stem.removesuffix("_raw_1.2x")
    else:
        if not args.text:
            p.error("text is required unless --degrade-only is used")
        name = args.name or slugify(args.text)
        x, sr = tts(args.text), SAMPLE_RATE
        raw = AUDIO_DIR / f"{name}_raw_1.2x.wav"  # untouched ElevenLabs output, reused by --degrade-only
        write_wav(raw, x, sr)
        print(f"raw   -> {raw}")

    x = time_stretch(x, args.speed / API_MAX_SPEED, sr)
    clean = AUDIO_DIR / f"{name}_{args.speed:g}x_clean.wav"
    write_wav(clean, x, sr)
    print(f"clean -> {clean}")

    out = AUDIO_DIR / f"{name}_{args.speed:g}x_grit{args.grit:g}_noise{args.noise:g}_bad.wav"
    write_wav(out, degrade(x, sr, args.seed, args.noise, args.grit), sr)
    print(f"bad   -> {out}")


if __name__ == "__main__":
    main()
