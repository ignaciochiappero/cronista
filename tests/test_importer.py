"""Transcribing a file the user drops on the window: any format ffmpeg reads, decoded to the model's audio."""
import shutil
import subprocess
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest

from cronista.importer import Cancelled, ImportFailed, decode_audio, probe, read_report, transcribe_file

ffmpeg = shutil.which("ffmpeg")
needs_ffmpeg = pytest.mark.skipif(ffmpeg is None, reason="needs ffmpeg")

SINE = ["-f", "lavfi", "-i", "sine=frequency=440:duration=3"]
PICTURE = ["-f", "lavfi", "-i", "color=c=0x223344:size=320x240:rate=25:duration=3"]


def make(folder: Path, name: str, *arguments: str) -> Path:
    path = folder / name
    subprocess.run([ffmpeg, "-v", "error", "-y", *arguments, str(path)], check=True)
    return path


class FakeModel:
    """Hears "A tone." in every chunk it is given."""

    def __init__(self):
        self.chunks = 0

    def with_timestamps(self):
        return self

    def recognize(self, audio, sample_rate):
        self.chunks += 1
        return SimpleNamespace(tokens=[" A", " tone", "."], timestamps=[0.2, 0.4, 0.6])


def test_reads_ffmpegs_report_on_a_file():
    # What `ffmpeg -i` says about a song with its album's picture: the picture is not a video.
    song = read_report("""Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'C:\\Music\\song (live).m4a':
  Metadata:
    title           : Stream #0:9: Video: not a stream
  Duration: 01:02:03.50, start: 0.000000, bitrate: 128 kb/s
  Stream #0:0[0x1](und): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 125 kb/s (default)
      Metadata:
        handler_name    : SoundHandler
  Stream #0:1[0x0]: Video: png, rgb24(pc, gbr/unknown/unknown), 600x600, 90k tbr, 90k tbn (attached pic)
At least one output file must be specified""")
    assert song.duration == 3723.5 and song.has_audio and not song.has_video

    live = read_report("""Input #0, matroska,webm, from 'call.webm':
  Duration: N/A, start: 0.000000, bitrate: N/A
  Stream #0:0(eng): Video: vp9 (Profile 0), yuv420p(tv, bt709), 1280x720, SAR 1:1 DAR 16:9, 30 fps (default)
  Stream #0:1(eng): Audio: opus, 48000 Hz, stereo, fltp (default)""")
    assert live.duration == 0.0 and live.has_audio and live.has_video

    with pytest.raises(ImportFailed, match="can't read"):
        read_report("notes.mp3: Invalid data found when processing input")


@needs_ffmpeg
def test_sound_pictures_and_cover_art_are_told_apart(tmp_path):
    audio = probe(ffmpeg, make(tmp_path, "talk.flac", *SINE))
    assert audio.has_audio and not audio.has_video
    assert audio.duration == pytest.approx(3.0, abs=0.05)

    video = probe(ffmpeg, make(tmp_path, "clip.mkv", *PICTURE, *SINE, "-c:v", "mpeg4", "-c:a", "aac"))
    assert video.has_audio and video.has_video

    song = probe(ffmpeg, make(tmp_path, "song.m4a", *SINE, "-f", "lavfi", "-i", "color=c=red:size=64x64",
                              "-map", "0:a", "-map", "1:v", "-frames:v", "1", "-c:a", "aac", "-c:v", "png",
                              "-disposition:v", "attached_pic"))
    assert song.has_audio and not song.has_video

    screen = probe(ffmpeg, make(tmp_path, "screen.mp4", *PICTURE, "-c:v", "mpeg4"))
    assert screen.has_video and not screen.has_audio


@needs_ffmpeg
def test_a_file_that_is_not_sound_or_video_cannot_be_read(tmp_path):
    notes = tmp_path / "notes.mp3"
    notes.write_text("not a song", encoding="utf-8")
    with pytest.raises(ImportFailed, match="can't read"):
        probe(ffmpeg, notes)


@needs_ffmpeg
def test_any_format_becomes_the_models_audio(tmp_path):
    stereo = make(tmp_path, "stereo.wav", "-f", "lavfi", "-i", "sine=frequency=300:duration=2:sample_rate=44100",
                  "-ac", "2")
    target = tmp_path / "audio-16k.f32"
    done = []
    decode_audio(ffmpeg, stereo, target, progress=done.append, stopped=lambda: False)
    samples = np.fromfile(target, np.float32)
    assert abs(len(samples) - 32_000) < 400  # 2 s at 16 kHz, one channel
    assert 0.05 < np.abs(samples).max() <= 1.0
    assert done[-1] == pytest.approx(2.0, abs=0.05)  # how many seconds have been read, the last time all of them


@needs_ffmpeg
def test_reading_stops_when_asked(tmp_path):
    long = make(tmp_path, "long.flac", "-f", "lavfi", "-i", "sine=duration=1200")
    target = tmp_path / "audio-16k.f32"
    with pytest.raises(Cancelled):
        decode_audio(ffmpeg, long, target, progress=lambda seconds: None, stopped=lambda: True)
    assert not target.exists()


@needs_ffmpeg
def test_a_file_is_transcribed_and_its_decoded_audio_removed(tmp_path):
    clip = make(tmp_path, "clip.mkv", *PICTURE, *SINE, "-c:v", "mpeg4", "-c:a", "flac")
    folder = tmp_path / "transcript"
    folder.mkdir()
    stages, lines = [], []
    result = transcribe_file(FakeModel(), ffmpeg, clip, folder,
                             progress=lambda stage, done, total: stages.append((stage, round(total, 1))),
                             lines=lines.extend, stopped=lambda: False)
    assert result.has_video and result.duration == pytest.approx(3.0, abs=0.05)
    assert [segment.text for segment in result.segments] == ["A tone."]
    assert [segment.text for segment in lines] == ["A tone."]  # each chunk's lines, as soon as it is done
    assert stages[0] == ("reading", 3.0) and stages[-1] == ("transcribing", 3.0)
    assert list(folder.iterdir()) == []  # the decoded audio does not stay


@needs_ffmpeg
def test_in_high_quality_the_refiner_writes_the_words_again(tmp_path):
    talk = make(tmp_path, "talk.flac", *SINE)
    folder = tmp_path / "transcript"
    folder.mkdir()
    lines = []

    def refine(audio, tokens, timestamps, sample_rate):
        return [" A", " clear", " tone."], [0.2, 0.4, 0.6]

    result = transcribe_file(FakeModel(), ffmpeg, talk, folder, progress=lambda *_: None, lines=lines.extend,
                             stopped=lambda: False, refine=refine)
    assert [segment.text for segment in result.segments] == ["A clear tone."]
    assert [segment.text for segment in lines] == ["A clear tone."]  # what shows while it is written, too


@needs_ffmpeg
def test_a_video_without_sound_says_so(tmp_path):
    screen = make(tmp_path, "screen.mp4", *PICTURE, "-c:v", "mpeg4")
    with pytest.raises(ImportFailed, match="no sound"):
        transcribe_file(FakeModel(), ffmpeg, screen, tmp_path, progress=lambda *_: None,
                        lines=lambda _: None, stopped=lambda: False)


@needs_ffmpeg
def test_a_stop_during_the_transcript_ends_it(tmp_path):
    talk = make(tmp_path, "talk.flac", *SINE)
    folder = tmp_path / "transcript"
    folder.mkdir()
    model = FakeModel()
    with pytest.raises(Cancelled):  # asked while the model worked on the only chunk: nothing is kept
        transcribe_file(model, ffmpeg, talk, folder, progress=lambda *_: None, lines=lambda _: None,
                        stopped=lambda: model.chunks > 0)
    assert model.chunks == 1
    assert list(folder.iterdir()) == []
