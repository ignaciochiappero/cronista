"""Loads Parakeet TDT 0.6B v3 (int8) through onnx-asr, CPU only, with a hard thread cap; and, for high
quality, Canary 1B v2 (int8), which is downloaded when the user first asks for it (see canary_pass).

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

from ..downloads import RemoteFile

MODEL_NAME = "nemo-parakeet-tdt-0.6b-v3"
# Next to the app when running from source; inside the bundle once packaged with PyInstaller.
APP_ROOT = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parents[2]))
MODEL_DIR = APP_ROOT / "models" / "parakeet-tdt-0.6b-v3"
CANARY_NAME = "nemo-canary-1b-v2"
# Canary is not in the installer. The files of this revision of istupakov's int8 export (CC BY 4.0, as
# NVIDIA's model) are downloaded the first time the user chooses high quality, each checked against its
# size and SHA-256 here: 1,029,326,602 bytes in all.
_CANARY_AT = "https://huggingface.co/istupakov/canary-1b-v2-onnx/resolve/5ebc1520cef7b6b318b3526ad17adbfe00bc1bfc"
CANARY_FILES = [
    RemoteFile("config.json", f"{_CANARY_AT}/config.json", 68,
               "f90ace8e35326dcd47c7330b230644fa0835083ed1e89e2f59aa08ba10d74f54"),
    RemoteFile("vocab.txt", f"{_CANARY_AT}/vocab.txt", 208_022,
               "2c9efe6104fd29522ea27ce0e3aef5d37c690af4e5a4232e643e23ca403ffea3"),
    RemoteFile("decoder-model.int8.onnx", f"{_CANARY_AT}/decoder-model.int8.onnx", 170_040_374,
               "52d83aa7aad41fbbe4f9dfcd341d784735a6eb4c6eb0d3290fc27a0d8ac39abf"),
    RemoteFile("encoder-model.int8.onnx", f"{_CANARY_AT}/encoder-model.int8.onnx", 859_078_138,
               "6d96e9945898e5ace48f4efecd459ca1df81859730be27b8af6b197639403ee1"),
]

_blas_limit = None  # kept alive: threadpool_limits restores the old value when collected


def _session_options(threads: int) -> rt.SessionOptions:
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
    return options


def load_parakeet(threads: int = 4):
    return onnx_asr.load_model(
        MODEL_NAME,
        path=MODEL_DIR,
        quantization="int8",
        sess_options=_session_options(threads),
        providers=["CPUExecutionProvider"],
    )


def load_canary(folder: Path, threads: int = 4):
    """Canary from the folder it was downloaded to: 2.6 s and about 1 GB more memory on a 4-core laptop."""
    return onnx_asr.load_model(
        CANARY_NAME,
        path=folder,
        quantization="int8",
        sess_options=_session_options(threads),
        providers=["CPUExecutionProvider"],
    )
