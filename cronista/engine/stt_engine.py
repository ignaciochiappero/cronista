"""Loads Parakeet TDT 0.6B v3 (int8) through onnx-asr, CPU only, with a hard thread cap.

The cap simulates the minimum machine we target (4 cores, integrated graphics): the model never
touches the GPU and should never keep more than `threads` cores busy. Two things broke the cap
in the first live run (peak ~6.6 threads):
- onnx-asr opens two sessions for Parakeet (encoder and decoder_joint), each with its own pool,
  and ONNX Runtime keeps idle pool threads spinning. The decoder is called once per audio frame
  from a Python loop, so the spinning never stopped. Spinning is turned off below.
- numpy's BLAS threads (used by the CPU preprocessing) default to every core; threadpoolctl caps them.
"""
from __future__ import annotations

import sys
from pathlib import Path

import onnx_asr
import onnxruntime as rt
from threadpoolctl import threadpool_limits

MODEL_NAME = "nemo-parakeet-tdt-0.6b-v3"
# Next to the app when running from source; inside the bundle once packaged with PyInstaller.
APP_ROOT = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parents[2]))
MODEL_DIR = APP_ROOT / "models" / "parakeet-tdt-0.6b-v3"

_blas_limit = None  # kept alive: threadpool_limits restores the old value when collected


def load_parakeet(threads: int = 4):
    global _blas_limit
    _blas_limit = threadpool_limits(limits=threads)

    options = rt.SessionOptions()
    options.intra_op_num_threads = threads
    options.inter_op_num_threads = 1
    options.add_session_config_entry("session.intra_op.allow_spinning", "0")
    options.add_session_config_entry("session.inter_op.allow_spinning", "0")
    # The arena keeps every buffer it ever grew to. Off, the model took 1224 MB instead of 1629 MB
    # on Linux (WSL, Ubuntu 26.04) at the same speed.
    options.enable_cpu_mem_arena = False
    return onnx_asr.load_model(
        MODEL_NAME,
        path=MODEL_DIR,
        quantization="int8",
        sess_options=options,
        providers=["CPUExecutionProvider"],
    )
