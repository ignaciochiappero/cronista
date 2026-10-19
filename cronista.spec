# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller spec for Cronista, adapted from escriba's transcriptor-full.spec.

Build:  .venv\\Scripts\\pyinstaller.exe cronista.spec --noconfirm --clean
Result: dist\\Cronista\\Cronista.exe plus _internal\\ (the app, Python, the model and ffmpeg).
"""
import os
import shutil

from PyInstaller.utils.hooks import collect_data_files, copy_metadata

ROOT = os.path.abspath(".")
FFMPEG_EXE = shutil.which("ffmpeg")
if not FFMPEG_EXE:
    raise SystemExit("ffmpeg.exe not found on PATH. Install it with: winget install ffmpeg")
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
# FFmpeg's license and build details go with it (GPL v3): the gyan.dev build keeps them one folder up from bin.
for name, shipped in (("LICENSE", "LICENSE.txt"), ("README.txt", "README.txt")):
    source = os.path.join(os.path.dirname(os.path.dirname(FFMPEG_EXE)), name)
    if not os.path.isfile(source):
        raise SystemExit(f"FFmpeg's {name} is missing next to its bin folder: {source}")
    shipped_copy = os.path.join(ROOT, "build", "ffmpeg-notices", shipped)
    os.makedirs(os.path.dirname(shipped_copy), exist_ok=True)
    shutil.copyfile(source, shipped_copy)
    datas.append((shipped_copy, "ffmpeg"))
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
