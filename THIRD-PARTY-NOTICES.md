# Third-party notices

Cronista's own code is under the Apache License 2.0 (see `LICENSE` and `NOTICE`). The installer
on the Releases page also contains the software below, each under its own license.

## FFmpeg

The installer bundles `ffmpeg.exe` (FFmpeg 9.0, version `n9.0.2-28-g669f5ab083`, the static
`win64-lgpl` build from [BtbN/FFmpeg-Builds](https://github.com/BtbN/FFmpeg-Builds)), licensed
under the **GNU Lesser General Public License version 3** or later. Cronista runs it as a separate
program, to capture the screen and the sound and to encode them.

- License text: installed next to it, as `_internal\ffmpeg\LICENSE.txt`.
- Build details (its version and configuration, as `ffmpeg -version` prints them):
  `_internal\ffmpeg\BUILD.txt`.
- Source code: FFmpeg at commit
  [669f5ab083](https://github.com/FFmpeg/FFmpeg/commit/669f5ab083c7732c10f1ce9e4f1915960dd88457)
  (branch `release/9.0`). The libraries linked into it come from the repositories and commits
  named by the build scripts, BtbN/FFmpeg-Builds at commit
  [63ac3265b0](https://github.com/BtbN/FFmpeg-Builds/tree/63ac3265b0789ea16c7b1f8351cd2ad195a28722)
  (`scripts.d`). The build is `ffmpeg-n9.0.2-28-g669f5ab083-win64-lgpl-9.0.zip`, from that
  project's release `autobuild-2026-10-19-13-06`.

The installers published for versions up to 0.4.0 bundled gyan.dev's "full" build of FFmpeg 9.0
instead, under the GPL version 3; the notices installed with them describe it.

To use another FFmpeg, replace `_internal\ffmpeg\ffmpeg.exe` with a build that has the
`gfxcapture` filter and the `h264_mf` encoder (Media Foundation), the two Cronista needs.

## Parakeet TDT 0.6B v3

The speech model, by NVIDIA, licensed under
[CC BY 4.0](https://creativecommons.org/licenses/by/4.0/):
[nvidia/parakeet-tdt-0.6b-v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3). Cronista uses
its ONNX conversion (int8) by [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx),
through onnx-asr. The model files are unmodified.

## Canary 1B v2

Not in the installer: Cronista downloads it when you choose **High** transcription quality. The
speech model, by NVIDIA, licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/):
[nvidia/canary-1b-v2](https://huggingface.co/nvidia/canary-1b-v2). Cronista uses its ONNX conversion
(int8) by [istupakov](https://huggingface.co/istupakov/canary-1b-v2-onnx), revision `5ebc1520`,
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
