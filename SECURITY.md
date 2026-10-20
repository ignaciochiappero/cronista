# Security

Cronista records your screen and what is said. This page says exactly what it records, where it keeps it, and what it runs, so you can decide whether to trust it.

## Report a vulnerability

Please report it privately, not in a public issue: use **Report a vulnerability** in the Security tab of this repository. Fixes are released for the latest version only.

## In short

| Question | Answer |
|----------|--------|
| Does it send anything over the network? | No. The speech model runs on your computer. There is no update check, no telemetry, and no account. |
| What does it record? | Only between pressing the red button and pressing it again: your whole main screen, the microphone you chose, and, with **Computer sound** on, what your computer plays. |
| Does it need administrator rights? | No. It installs for the current user only. |
| Is the installer signed? | Not yet. Windows SmartScreen warns about it. Check the download against its SHA-256 before running it. |

## What it stores

| Where | What |
|-------|------|
| `Videos\Cronista\<date and time>\` | One folder per recording: `recording.mp4`, `transcript.srt`, `transcript.txt`, `recorder.log` (FFmpeg's messages while recording), and `transcript.docx`, `transcript.pdf` or `recording-subtitled.mp4` once you make them. While a recording runs, the folder also holds its parts (`screen.mp4`, `audio.m4a`) and the audio the speech model reads (`audio-16k*.f32`, one file per audio source); they are deleted when it stops. While a subtitled copy is made, `subtitles.ass` is there too, and is deleted when it is done. |
| `%LOCALAPPDATA%\Cronista\library.db` | The history: each recording's title, date, length, size, folder, the names of its audio sources, and its full transcript (with which source said each phrase), which is what the search looks through. |
| `%LOCALAPPDATA%\Cronista\settings.json` | The microphone you chose (its Windows device id), the quality, whether computer sound is on, the names you gave the two audio sources, whether recordings get a subtitled copy, the language, and whether the guide was shown. |
| `%LOCALAPPDATA%\Cronista\cronista.log` | A line each time the app starts, and the details of any error. Not what was said. |

None of these files are encrypted. The recordings and the history hold what was said in them: anyone who can read your user profile can read them too.

**Delete** removes a recording from the history together with its folder. It only ever deletes a folder that sits directly in `Videos\Cronista`. Uninstalling the app keeps your recordings and the history; delete those folders yourself if you want them gone.

## Programs it runs

| Program | Used for | Network |
|---------|----------|---------|
| `ffmpeg.exe`, installed with the app | Capturing the screen, mixing and encoding the sound, putting both together in `recording.mp4`, and making the subtitled copy | None |
| Windows Explorer | **Show in folder**, and showing an exported file | None |
| `clip.exe` (part of Windows) | **Copy text** | None |
| Your default video player | **Play** | Whatever that player does |

Arguments are passed directly, without a shell. The microphone and the computer's sound are read through Windows' own audio interface (WASAPI) and handed to FFmpeg through named pipes that only exist while a recording runs.

## The window

The window shows a page that ships with the app and loads nothing from the internet. The page can call the app's 16 functions, one per action (start, stop, rename, export, and so on), and nothing else. Titles and transcripts are always shown as text, never read as HTML.

## Verify a download

Each release lists the SHA-256 of its installer, and has it in `SHA256SUMS.txt`. Compare it with the file you downloaded before you run it:

```powershell
Get-FileHash .\Cronista-Setup-*.exe -Algorithm SHA256
```

You can also build the installer yourself from the source: see [Build the installer](README.md#build-the-installer).

## Known limits

- **The installer is not code-signed.** Windows SmartScreen shows a warning the first time you run it. Verify the checksum, or build from source.
- **There is no automatic update.** You install a new version by downloading it.
- **The files are not encrypted.** See "What it stores" above.
- **Everything on the main screen is recorded,** including notifications and any window that opens over the one you meant to show.
- **Computer sound records everything the computer plays,** not only the call or the video you have in mind: notification sounds too.
- **Only Windows 11 is tested.**
