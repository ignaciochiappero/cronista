"""Stands in for ffmpeg in tests: no screen, no microphone.

Streams a WAV on stdout as 16 kHz mono float32, faster than real time, then silence until it reads
"q" on stdin (like the real recorders), and writes a placeholder video file. With --audio (once per
audio source) it also reads those pipes, as ffmpeg reads the sources' pipes, writes "first frame"
to stderr after --first-frame-after seconds, as the screen capture reports its first frame, and
when it stops writes how many bytes each pipe delivered to <out>.audio-bytes.
"""
import argparse
import os
import sys
import threading
import time
import wave
from pathlib import Path

import numpy as np

BLOCK = 1_600  # 100 ms at 16 kHz

parser = argparse.ArgumentParser()
parser.add_argument("--wav", required=True)
parser.add_argument("--out", required=True)
parser.add_argument("--speed", type=float, default=4.0)
parser.add_argument("--audio", action="append", default=[])
parser.add_argument("--first-frame-after", type=float)
args = parser.parse_args()

with wave.open(args.wav) as wav:
    audio = np.frombuffer(wav.readframes(wav.getnframes()), np.int16).astype(np.float32) / 32768
Path(args.out).write_bytes(b"placeholder video")

stop = threading.Event()


def watch_stdin() -> None:
    while True:
        char = os.read(0, 1)
        if not char:
            return
        if char in b"qQ":
            stop.set()
            return


received = [0] * len(args.audio)


def read_audio(index: int, path: str) -> None:
    with open(path, "rb", buffering=0) as pipe:
        while chunk := pipe.read(65536):
            received[index] += len(chunk)


def report_first_frame(delay: float) -> None:
    time.sleep(delay)
    sys.stderr.write("first frame\n")
    sys.stderr.flush()


threading.Thread(target=watch_stdin, daemon=True).start()
for index, path in enumerate(args.audio):
    threading.Thread(target=read_audio, args=(index, path), daemon=True).start()
if args.first_frame_after is not None:
    threading.Thread(target=report_first_frame, args=(args.first_frame_after,), daemon=True).start()
sys.stderr.write("[info] fake recorder started\n")
sys.stderr.flush()

out = sys.stdout.buffer
silence = np.zeros(BLOCK, np.float32).tobytes()
for start in range(0, len(audio), BLOCK):
    if stop.is_set():
        break
    out.write(audio[start : start + BLOCK].tobytes())
    out.flush()
    time.sleep(0.1 / args.speed)
deadline = time.monotonic() + 60
while not stop.is_set() and time.monotonic() < deadline:
    out.write(silence)
    out.flush()
    time.sleep(0.1 / args.speed)
if args.audio:
    Path(args.out + ".audio-bytes").write_text("\n".join(str(count) for count in received))
