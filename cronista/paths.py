"""Where Cronista keeps things. No heavy imports here: the packaged app reads it before anything else."""
from __future__ import annotations

import os
from pathlib import Path

IS_WINDOWS = os.name == "nt"


def data_dir() -> Path:
    """The history database, settings and the log."""
    if IS_WINDOWS:
        return Path(os.environ.get("LOCALAPPDATA", Path.home() / "AppData" / "Local")) / "Cronista"
    return Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share")) / "cronista"


def recordings_dir() -> Path:
    """One folder per recording."""
    return Path.home() / "Videos" / "Cronista"
