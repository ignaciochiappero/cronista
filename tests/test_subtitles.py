import os
import shutil
import subprocess
from pathlib import Path

import numpy as np
import pytest

from cronista.engine.transcript import TranscriptionSegment
from cronista.subtitles import SUBTITLED_NAME, ass_document, burn_subtitles

CALL = [TranscriptionSegment(0.2, 1.5, "Let's start {now}.", "mic"),
        TranscriptionSegment(1.5, 2.9, "Sure, the screens are ready.", "computer")]
NAMES = {"mic": "Me", "computer": "Ana"}


def dialogue(document: str) -> list[str]:
    return [line for line in document.splitlines() if line.startswith("Dialogue:")]


def test_each_phrase_is_a_subtitle_with_its_speaker_in_their_color():
    lines = dialogue(ass_document(CALL, NAMES))
    assert len(lines) == 2
    assert lines[1].startswith("Dialogue: 0,0:00:01.50,0:00:02.90,Default,")
    assert lines[1].endswith(r"{\c&H00FFB48A&\b1}Ana:{\c&H00FFFFFF&\b0} Sure, the screens are ready.")
    assert r"{\c&H007F8AFF&\b1}Me:" in lines[0]
    assert "{now}" not in lines[0] and "(now)" in lines[0]  # braces would start an override block


def test_without_two_speakers_the_subtitles_are_only_what_was_said():
    lines = dialogue(ass_document(CALL, None))
    assert lines[0].endswith(",,Let's start (now).")
    assert "PlayResY: 1080" in ass_document(CALL, None)


ffmpeg = shutil.which("ffmpeg")


@pytest.mark.skipif(os.name != "nt" or ffmpeg is None or shutil.which("ffprobe") is None,
                    reason="needs Windows (Media Foundation), ffmpeg and ffprobe")
def test_the_copy_has_the_subtitles_burned_in_and_the_same_sound(tmp_path: Path):
    subprocess.run([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "color=c=0x223344:size=640x360:rate=30:duration=3",
                    "-f", "lavfi", "-i", "sine=duration=3", "-c:v", "mpeg4", "-c:a", "aac",
                    str(tmp_path / "recording.mp4")], check=True)
    path = burn_subtitles(ffmpeg, tmp_path, CALL, NAMES)
    assert path == tmp_path / SUBTITLED_NAME and path.exists()
    assert not (tmp_path / "subtitles.ass").exists()  # only there while the copy is made

    def frame(video: Path, at: float) -> np.ndarray:
        raw = subprocess.run([ffmpeg, "-v", "error", "-ss", str(at), "-i", str(video), "-frames:v", "1",
                              "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"], capture_output=True, check=True).stdout
        return np.frombuffer(raw, np.uint8).reshape(360, 640).astype(int)

    before, after = frame(tmp_path / "recording.mp4", 2.0), frame(path, 2.0)
    bottom = slice(270, 360)
    assert np.abs(after[bottom] - before[bottom]).mean() > 3  # something was drawn at the bottom
    assert np.abs(after[:150] - before[:150]).mean() < 3  # and nowhere else
    streams = subprocess.run([shutil.which("ffprobe"), "-v", "error", "-show_entries", "stream=codec_type",
                              "-of", "csv=p=0", str(path)], capture_output=True, text=True, check=True).stdout.split()
    assert streams == ["video", "audio"]


@pytest.mark.skipif(os.name != "nt" or ffmpeg is None, reason="needs Windows (Media Foundation) and ffmpeg")
def test_making_the_copy_says_how_much_of_the_video_is_done(tmp_path: Path):
    subprocess.run([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "color=c=0x223344:size=320x180:rate=30:duration=4",
                    "-f", "lavfi", "-i", "sine=duration=4", "-c:v", "mpeg4", "-c:a", "aac",
                    str(tmp_path / "recording.mp4")], check=True)
    done: list[float] = []
    burn_subtitles(ffmpeg, tmp_path, CALL, NAMES, progress=done.append)
    assert done and done == sorted(done)  # seconds of the video made so far, as ffmpeg says them
    assert 3.5 < done[-1] <= 4.1


@pytest.mark.skipif(os.name != "nt" or ffmpeg is None or shutil.which("ffprobe") is None,
                    reason="needs Windows (Media Foundation), ffmpeg and ffprobe")
def test_a_transcribed_video_gets_its_copy_in_cronistas_folder_and_is_left_alone(tmp_path: Path):
    # Any format: here odd-sized pictures and FLAC sound in a Matroska file, with a subtitle track
    # an MP4 could not hold.
    downloads = tmp_path / "Downloads"
    downloads.mkdir()
    source = downloads / "Demo [final].mkv"
    (tmp_path / "talk.srt").write_text("1\n00:00:00,000 --> 00:00:01,000\nHi\n", encoding="utf-8")
    subprocess.run([ffmpeg, "-v", "error", "-f", "lavfi", "-i", "color=c=0x223344:size=641x361:rate=25:duration=3",
                    "-f", "lavfi", "-i", "sine=duration=3", "-i", str(tmp_path / "talk.srt"),
                    "-c:v", "mpeg4", "-c:a", "flac", "-c:s", "srt", str(source)], check=True)
    before = source.read_bytes()
    folder = tmp_path / "Cronista"
    folder.mkdir()
    path = burn_subtitles(ffmpeg, folder, CALL, None, source=source)
    assert path == folder / "Demo [final]-subtitled.mp4" and path.exists()
    assert source.read_bytes() == before and sorted(p.name for p in downloads.iterdir()) == ["Demo [final].mkv"]
    streams = subprocess.run([shutil.which("ffprobe"), "-v", "error", "-show_entries", "stream=codec_type,codec_name",
                              "-of", "csv=p=0", str(path)], capture_output=True, text=True, check=True).stdout.split()
    assert streams == ["h264,video", "aac,audio"]
