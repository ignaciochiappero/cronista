# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller spec for Cronista, adapted from escriba's transcriptor-full.spec.

Build:  .venv\\Scripts\\pyinstaller.exe cronista.spec --noconfirm --clean
Result: dist\\Cronista\\Cronista.exe plus _internal\\ (the app, Python, the model and ffmpeg).

The bundled ffmpeg.exe is the one the FFMPEG_EXE environment variable points to, else the one on
PATH. It must be an LGPL build, such as BtbN's win64-lgpl (https://github.com/BtbN/FFmpeg-Builds).
"""
import os
import shutil
import subprocess

from PyInstaller.utils.hooks import collect_data_files, copy_metadata

ROOT = os.path.abspath(".")
FFMPEG_EXE = os.environ.get("FFMPEG_EXE") or shutil.which("ffmpeg")
if not FFMPEG_EXE or not os.path.isfile(FFMPEG_EXE):
    raise SystemExit("ffmpeg.exe not found. Set FFMPEG_EXE to an LGPL build's bin\\ffmpeg.exe, or put one on PATH.")
FFMPEG_VERSION = subprocess.run([FFMPEG_EXE, "-hide_banner", "-version"], capture_output=True, text=True, check=True).stdout
# The installer's notices say LGPL: a build configured with GPL or non-free parts must not slip in.
if "--enable-gpl" in FFMPEG_VERSION or "--enable-nonfree" in FFMPEG_VERSION:
    raise SystemExit(f"{FFMPEG_EXE} is a GPL or non-free build. Set FFMPEG_EXE to an LGPL one.")
MODEL_DIR = os.path.join(ROOT, "models", "parakeet-tdt-0.6b-v3")
if not os.path.isdir(MODEL_DIR):
    raise SystemExit(f"The speech model is missing: {MODEL_DIR}")

datas = [
    (os.path.join(ROOT, "ui"), "ui"),
    (MODEL_DIR, "models/parakeet-tdt-0.6b-v3"),
    (FFMPEG_EXE, "ffmpeg"),  # only ffmpeg.exe: the app never calls ffprobe or ffplay
    (os.path.join(ROOT, "LICENSE"), "."),
    (os.path.join(ROOT, "NOTICE"), "."),
    (os.path.join(ROOT, "THIRD-PARTY-NOTICES.md"), "."),
]
# FFmpeg's license and build details go with it: BtbN's builds keep LICENSE.txt one folder up from bin
# and have no README, so the configuration comes from the binary itself (ffmpeg -version).
FFMPEG_NOTICES = os.path.join(ROOT, "build", "ffmpeg-notices")
os.makedirs(FFMPEG_NOTICES, exist_ok=True)
license_source = os.path.join(os.path.dirname(os.path.dirname(FFMPEG_EXE)), "LICENSE.txt")
if not os.path.isfile(license_source):
    raise SystemExit(f"FFmpeg's LICENSE.txt is missing next to its bin folder: {license_source}")
shutil.copyfile(license_source, os.path.join(FFMPEG_NOTICES, "LICENSE.txt"))
with open(os.path.join(FFMPEG_NOTICES, "BUILD.txt"), "w", encoding="utf-8") as build_details:
    build_details.write(FFMPEG_VERSION.split("\nExiting with exit code")[0].rstrip() + "\n")
datas += [(os.path.join(FFMPEG_NOTICES, name), "ffmpeg") for name in ("LICENSE.txt", "BUILD.txt")]
datas += collect_data_files("onnx_asr")  # the mel preprocessors it loads from package data
datas += collect_data_files("docx")  # python-docx's default template
datas += collect_data_files("reportlab")  # its font metrics and fonts; the PDF embeds Segoe UI, but just in case
datas += copy_metadata("onnx-asr")  # onnx_asr reads its own version from the package metadata on import

a = Analysis(
    [os.path.join(ROOT, "app.py")],
    pathex=[ROOT],
    binaries=[],
    datas=datas,
    hiddenimports=[
        "webview.platforms.edgechromium", "webview.platforms.winforms", "clr_loader", "pythonnet",
        "pycaw.pycaw", "comtypes",
    ],
    excludes=["tkinter", "matplotlib", "pytest", "IPython"],
    noarchive=False,
)
pyz = PYZ(a.pure)
exe = EXE(
    pyz,
    a.scripts,
    [],
    exclude_binaries=True,
    name="Cronista",
    console=False,
    upx=False,  # UPX-packed DLLs trip antivirus heuristics and can break onnxruntime
    icon=os.path.join(ROOT, "assets", "cronista.ico"),
)
coll = COLLECT(exe, a.binaries, a.datas, upx=False, name="Cronista")
