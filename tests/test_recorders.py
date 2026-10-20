import os
import shutil
import subprocess
from pathlib import Path

import pytest

from cronista.engine.recorders import RecorderSettings, WindowsRecorder, pick_microphone

MIC_PIPE = r"\\.\pipe\cronista-test-mic"
COMPUTER_PIPE = r"\\.\pipe\cronista-test-computer"
DEVICES = [{"id": "{mic-a}", "name": "Mic A"}, {"id": "{mic-b}", "name": "Mic B"}]


class FakeSource:
    def __init__(self, path: str, layout: str) -> None:
        self.path = path
        self.layout = layout

    def ffmpeg_input(self) -> list[str]:
        return ["-f", "f32le", "-sample_rate", "48000", "-ch_layout", self.layout, "-i", self.path]


MIC = FakeSource(MIC_PIPE, "mono")
COMPUTER = FakeSource(COMPUTER_PIPE, "stereo")


def maps(command: list[str]) -> list[str]:
    return [command[i + 1] for i, arg in enumerate(command) if arg == "-map"]


def processes(tmp_path: Path, sources) -> dict:
    return {process.name: process for process in WindowsRecorder("ffmpeg").processes(tmp_path, RecorderSettings(), sources)}


def test_the_screen_and_the_audio_are_separate_processes(tmp_path: Path):
    # In one ffmpeg, a still screen held the audio back: ffmpeg paces every input to the stream that
    # lags most, and a still screen sends no frames. In 20 s of still screen, 1 s of audio got through.
    found = processes(tmp_path, [MIC])
    screen, audio = found["screen"].command, found["audio"].command
    assert "-i" not in screen and not any("pipe" in arg for arg in screen)
    assert not any(arg.startswith("gfxcapture=") for arg in audio)
    assert found["audio"].speech and not found["screen"].speech


def test_the_screen_is_recorded_alone_and_reports_its_first_frame(tmp_path: Path):
    recorder = WindowsRecorder("ffmpeg")
    screen = processes(tmp_path, [MIC])["screen"].command
    graph = next(arg for arg in screen if arg.startswith("gfxcapture="))
    assert "showinfo" in graph and "nullsink" not in graph  # a sink waiting on frames aborted ffmpeg
    # showinfo writes at the info level (32); one "-" typed after the first frame takes it to 24.
    assert screen[screen.index("-loglevel") + 1] == "level+34"
    assert recorder.after_first_frame == b"-"
    assert screen[-1].endswith("screen.mp4") and "h264_mf" in screen
    # Lines from a real run of that graph:
    first = ("[Parsed_showinfo_3 @ 0000024f24f99440] [info] n:   0 pts:      0 "
             "pts_time:0       duration:      0 duration_time:0       fmt:d3d11")
    config = "[Parsed_showinfo_3 @ 0000024f24f99440] [info] config in time_base: 1/10000000, frame_rate: 30/1"
    assert recorder.screen_started(first)
    assert not recorder.screen_started(config)
    assert not recorder.screen_started("[info] Press [q] to stop, [?] for help")
    assert recorder.frame_report(config) and not recorder.frame_report("[info] Press [q] to stop, [?] for help")


def test_the_microphone_alone_goes_to_the_audio_file_and_the_model(tmp_path: Path):
    audio = processes(tmp_path, [MIC])["audio"].command
    assert audio.count("-i") == 1
    assert audio[audio.index(MIC_PIPE) - 1] == "-i"
    assert maps(audio) == ["0:a", "0:a"]
    assert not any("amix" in arg or "dshow" in arg for arg in audio)


def test_with_the_computer_sound_the_mix_goes_to_the_audio_file_and_the_model(tmp_path: Path):
    audio = processes(tmp_path, [MIC, COMPUTER])["audio"].command
    assert audio.count("-i") == 2
    assert audio.index(MIC_PIPE) < audio.index(COMPUTER_PIPE)
    graph = next(arg for arg in audio if "amix" in arg)
    assert "[0:a]" in graph and "[1:a]" in graph
    assert maps(audio) == ["[rec]", "[stt]"]
    assert any(arg.endswith("audio.m4a") for arg in audio)
    speech = audio[audio.index("[stt]"):]
    assert speech[-1] == "pipe:1"
    assert speech[speech.index("-ar") + 1] == "16000" and speech[speech.index("-ac") + 1] == "1"


needs_ffmpeg = pytest.mark.skipif(shutil.which("ffmpeg") is None, reason="needs ffmpeg on PATH")


def make_parts(folder: Path, *, screen: bool = True, audio: bool = True) -> None:
    if screen:
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=30:duration=2",
                        "-c:v", "mpeg4", str(folder / "screen.mp4")], check=True)  # libx264 is not in LGPL builds
    if audio:
        subprocess.run(["ffmpeg", "-v", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=2",
                        "-c:a", "aac", str(folder / "audio.m4a")], check=True)


def streams(path: Path) -> list[str]:
    probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "stream=codec_type", "-of", "csv=p=0", str(path)],
                           capture_output=True, text=True, check=True)
    return probe.stdout.split()


@needs_ffmpeg
def test_finishing_puts_the_screen_and_the_sound_in_one_file(tmp_path: Path):
    make_parts(tmp_path)
    WindowsRecorder("ffmpeg").finish(tmp_path)
    assert streams(tmp_path / "recording.mp4") == ["video", "audio"]
    assert not (tmp_path / "screen.mp4").exists() and not (tmp_path / "audio.m4a").exists()


@needs_ffmpeg
def test_finishing_keeps_what_there_is_when_a_part_is_missing(tmp_path: Path):
    make_parts(tmp_path, screen=False)
    WindowsRecorder("ffmpeg").finish(tmp_path)
    assert streams(tmp_path / "recording.mp4") == ["audio"]


@pytest.mark.skipif(os.name != "nt" or shutil.which("ffmpeg") is None, reason="needs Windows and ffmpeg")
def test_a_still_screen_stops_promptly_when_asked(tmp_path: Path):
    # gfxcapture only looks at a stop request when a frame comes, and a still screen sends none:
    # once "q" waited more than 30 s. The recorder's wake_screen() makes a change on the screen.
    import time

    from cronista.engine.recorders import wake_screen

    recorder = WindowsRecorder("ffmpeg")
    screen = recorder.processes(tmp_path, RecorderSettings(), [MIC])[0].command
    process = subprocess.Popen(screen, stdin=subprocess.PIPE, stderr=subprocess.DEVNULL,
                               creationflags=subprocess.CREATE_NO_WINDOW)
    time.sleep(2)
    process.stdin.write(b"q")
    process.stdin.flush()
    wake_screen()
    process.wait(timeout=10)
    assert streams(tmp_path / "screen.mp4") == ["video"]


def test_the_chosen_microphone_wins_by_id_or_by_its_old_saved_name():
    assert pick_microphone(DEVICES, "{mic-a}", "{mic-b}")["name"] == "Mic A"
    assert pick_microphone(DEVICES, "Mic A", "{mic-b}")["name"] == "Mic A"  # settings from before 0.2.0


def test_otherwise_the_windows_default_then_the_first():
    assert pick_microphone(DEVICES, None, "{mic-b}")["name"] == "Mic B"
    assert pick_microphone(DEVICES, "{gone}", "{mic-b}")["name"] == "Mic B"
    assert pick_microphone(DEVICES, None, None)["name"] == "Mic A"
    assert pick_microphone([], "{mic-a}", None) is None
