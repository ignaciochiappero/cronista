"""Stands in for the screen's ffmpeg in tests: reports its first frame on stderr after a moment,
writes a placeholder file, and stops when it reads "q" on stdin."""
import argparse
import os
import sys
import time
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("--out", required=True)
parser.add_argument("--first-frame-after", type=float, default=0.3)
args = parser.parse_args()

sys.stderr.write("fake screen started\n")
sys.stderr.flush()
time.sleep(args.first_frame_after)
sys.stderr.write("first frame\n")
sys.stderr.flush()
Path(args.out).write_bytes(b"placeholder screen")
while (char := os.read(0, 1)) and char not in b"qQ":
    pass
