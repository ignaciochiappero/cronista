"""Finds ffmpeg/ffprobe.

`find_ffmpeg` is escriba's backend/app/services/audio.py::_find_ffmpeg: the copy bundled with the
installed app first, then PATH, then the usual Windows install locations (winget, Scoop, C:\\ffmpeg).
`find_ffprobe` and `probe_duration` follow escriba's AudioService.get_duration.
"""
from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

# A packaged (windowed) app must not flash a console window for every ffmpeg call.
NO_WINDOW = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}


def find_ffmpeg() -> str | None:
    bundled = Path(getattr(sys, "_MEIPASS", "")) / "ffmpeg" / "ffmpeg.exe"
    if getattr(sys, "frozen", False) and bundled.is_file():
        return str(bundled)

    found = shutil.which("ffmpeg")
    if found:
        return found

    local_app_data = os.environ.get("LOCALAPPDATA", "")
    user_profile = os.environ.get("USERPROFILE", "")
    common_paths = [
        r"C:\ffmpeg\bin\ffmpeg.exe",
        r"C:\Program Files\ffmpeg\bin\ffmpeg.exe",
        os.path.join(local_app_data, "Microsoft", "WinGet", "Links", "ffmpeg.exe"),
    ]

    winget_packages = os.path.join(local_app_data, "Microsoft", "WinGet", "Packages")
    if os.path.isdir(winget_packages):
        for package in os.listdir(winget_packages):
            if "ffmpeg" in package.lower() or "gyan" in package.lower():
                for root, _dirs, files in os.walk(os.path.join(winget_packages, package)):
                    if "ffmpeg.exe" in files:
                        return os.path.join(root, "ffmpeg.exe")

    common_paths.append(os.path.join(user_profile, "scoop", "apps", "ffmpeg", "current", "bin", "ffmpeg.exe"))
    for path in common_paths:
        if os.path.isfile(path):
            return path
    return None


def find_ffprobe(ffmpeg: str) -> str:
    """ffprobe lives next to ffmpeg."""
    ffmpeg_path = Path(ffmpeg)
    return str(ffmpeg_path.with_name("ffprobe" + ffmpeg_path.suffix))


def probe_duration(ffprobe: str, media: Path) -> float | None:
    result = subprocess.run(
        [ffprobe, "-v", "quiet", "-show_entries", "format=duration", "-of", "csv=p=0", str(media)],
        capture_output=True,
        text=True,
        **NO_WINDOW,
    )
    try:
        return float(result.stdout.strip())
    except ValueError:
        return None
