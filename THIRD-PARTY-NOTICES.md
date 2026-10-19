# Third-party notices

Cronista's own code is under the Apache License 2.0 (see `LICENSE` and `NOTICE`). The installer
on the Releases page also contains the software below, each under its own license.

## FFmpeg

The installer bundles `ffmpeg.exe` (FFmpeg 9.0, the "full" static Windows build from
[gyan.dev](https://www.gyan.dev/ffmpeg/builds/)), licensed under the **GNU General Public License
version 3**. Cronista runs it as a separate program, to capture the screen and the sound and to
encode them.

- License text: installed next to it, as `_internal\ffmpeg\LICENSE.txt`.
- Build details (configuration and the libraries it includes): `_internal\ffmpeg\README.txt`.
- Source code: FFmpeg at commit
  [d32b387f2b](https://github.com/FFmpeg/FFmpeg/commit/d32b387f2b), as stated by the build. The
  build's own page lists where the sources of its external libraries are.

To use another FFmpeg, replace `_internal\ffmpeg\ffmpeg.exe` with a build that has the
`gfxcapture` filter and the `h264_mf` encoder (Media Foundation), the two Cronista needs.

## Parakeet TDT 0.6B v3

The speech model, by NVIDIA, licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/):
[nvidia/parakeet-tdt-0.6b-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3). Cronista uses
its ONNX conversion (int8) by [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
through onnx-asr. The model files are unmodified.

## Python and its packages

| Software | License |
|---|---|
| Python 3.11 | PSF License |
| onnx-asr | MIT |
| ONNX Runtime | MIT |
| NumPy | BSD-3-Clause (and compatible licenses for its parts) |
| pywebview | BSD-3-Clause |
| pythonnet, clr-loader | MIT |
| python-docx | MIT |
| lxml | BSD-3-Clause |
| ReportLab | BSD-3-Clause |
| pycaw | MIT |
| comtypes | MIT |
| psutil | BSD-3-Clause |
| threadpoolctl | BSD-3-Clause |
| PyInstaller (bootloader) | GPL v2 or later, with an exception that allows distributing the programs it bundles under any license |

The window is drawn by Microsoft Edge WebView2, which is part of Windows and not included.

## Test data

`tests/data/en.wav` and `tests/data/es.wav` are synthetic speech, made with the voices that come
with Windows (System.Speech: Microsoft Zira and Microsoft Sabina). They are only used by the tests.
