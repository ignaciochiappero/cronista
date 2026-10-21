import os
import sys
import threading
import time
from pathlib import Path

import pytest
from fakes import SilentEndpoint

from cronista.engine.recorders import RecorderProcess, RecorderSettings
from cronista.engine.session import RecordingSession
from cronista.engine.stt_engine import load_parakeet

DATA = Path(__file__).parent / "data"
HERE = Path(__file__).parent


class FakeRecorder:
    """Runs fake_recorder.py: the audio for the model on stdout. With audio sources it reads their
    pipes like ffmpeg and reports a first frame 0.3 s after starting, like the screen capture; with
    a screen process, fake_screen.py does the reporting, as the Windows recorder's screen does."""

    output_name = "recording.mp4"
    after_first_frame = b"-"  # the fakes ignore it, like ffmpeg ignores keys it doesn't know

    def __init__(self, wav: Path, *, second_wav: Path | None = None, audio_sources: bool = False,
                 screen_process: bool = False) -> None:
        self.wav = wav
        self.second_wav = second_wav  # on a second channel, as the computer's sound comes
        self.with_sources = audio_sources
        self.with_screen = screen_process

    def audio_sources(self, settings: RecorderSettings, gate, on_notice) -> list:
        if not self.with_sources:
            return []
        from cronista.engine.audio_sources import AudioSource

        return [AudioSource("microphone", SilentEndpoint(), 1, gate, lag_seconds=0.06),
                AudioSource("computer sound", SilentEndpoint(), 2, gate, lag_seconds=0.06)]

    def processes(self, folder: Path, settings: RecorderSettings, sources) -> list[RecorderProcess]:
        audio = [sys.executable, str(HERE / "fake_recorder.py"), "--wav", str(self.wav),
                 "--out", str(folder / self.output_name)]
        for source in sources:
            audio += ["--audio", source.path]
        channels = 1
        if self.second_wav:
            audio += ["--wav2", str(self.second_wav)]
            channels = 2
        if not self.with_screen:
            if sources:
                audio += ["--first-frame-after", "0.3"]
            return [RecorderProcess("recorder", audio, speech=True, speech_channels=channels)]
        screen = [sys.executable, str(HERE / "fake_screen.py"), "--out", str(folder / "screen.part")]
        return [RecorderProcess("screen", screen), RecorderProcess("audio", audio, speech=True, speech_channels=channels)]

    def finish(self, folder: Path) -> None:
        if self.with_screen:
            (folder / "finished").write_text("the parts were put together")

    @staticmethod
    def screen_started(line: str) -> bool:
        return line == "first frame"


class Events:
    def __init__(self) -> None:
        self.items: list[tuple[str, dict]] = []
        self.changed = threading.Condition()

    def __call__(self, kind: str, payload: dict) -> None:
        with self.changed:
            self.items.append((kind, payload))
            self.changed.notify_all()

    def wait_for(self, kind: str, timeout: float) -> None:
        with self.changed:
            assert self.changed.wait_for(lambda: any(k == kind for k, _ in self.items), timeout), f"no {kind} event"

    @property
    def kinds(self) -> list[str]:
        return [kind for kind, _ in self.items]


@pytest.fixture(scope="module")
def model():
    return load_parakeet(threads=4)


@pytest.mark.slow
def test_records_transcribes_live_and_finishes_with_the_final_pass(model, tmp_path):
    events = Events()
    session = RecordingSession(model, FakeRecorder(DATA / "es.wav"), RecorderSettings(), tmp_path / "rec", events)
    session.start()
    events.wait_for("recording", timeout=15)
    events.wait_for("phrase", timeout=60)
    time.sleep(2)  # the sample is 22.5 s, streamed at 4x: all of it is out by now
    result = session.stop()

    assert events.kinds[0] == "listening"
    assert "partial" in events.kinds
    assert events.kinds[-2:] == ["finalizing", "finished"]
    progress = [payload["progress"] for kind, payload in events.items if kind == "finalizing" and payload]
    assert progress and progress == sorted(progress) and progress[-1] == 1.0  # the final pass says how far it got
    assert "prueba de transcripción en vivo" in result.transcript.lower()
    assert result.duration_seconds > 20
    assert result.size_bytes > 0
    assert not any(path.exists() for path in session.tape_paths)  # the raw audio is gone once the final pass is done
    assert {segment.source for segment in result.segments} == {"mic"}


@pytest.mark.slow
def test_each_source_is_transcribed_apart_and_says_where_it_came_from(model, tmp_path):
    # The microphone speaks Spanish on channel 0 while the computer plays English on channel 1.
    events = Events()
    recorder = FakeRecorder(DATA / "es.wav", second_wav=DATA / "en.wav")
    session = RecordingSession(model, recorder, RecorderSettings(), tmp_path / "rec", events)
    session.start()
    events.wait_for("recording", timeout=15)
    events.wait_for("phrase", timeout=60)
    time.sleep(2.5)  # both samples (22.5 s and 15 s) are out at 4x by now
    result = session.stop()

    said = {source: " ".join(s.text for s in result.segments if s.source == source).lower()
            for source in ("mic", "computer")}
    assert "prueba de transcripción en vivo" in said["mic"]
    assert "live transcription test" in said["computer"]
    assert "transcription test" not in said["mic"]
    assert [s.start for s in result.segments] == sorted(s.start for s in result.segments)
    phrases = [payload for kind, payload in events.items if kind == "phrase"]
    assert {phrase["source"] for phrase in phrases} == {"mic", "computer"}
    assert 20 < result.duration_seconds < 40  # the stream's length, not both channels' samples added up
    assert not any(path.exists() for path in session.tape_paths)


CANARY = HERE.parent / "models" / "canary-1b-v2"


@pytest.mark.slow
@pytest.mark.skipif(not (CANARY / "encoder-model.int8.onnx").exists(), reason="needs Canary in models/canary-1b-v2")
def test_in_high_quality_each_source_is_written_again_in_its_own_language(model, tmp_path):
    from cronista.engine.canary_pass import CanaryPass
    from cronista.engine.stt_engine import load_canary

    canary = load_canary(CANARY)
    made = []

    def refiner():
        made.append(CanaryPass(canary))
        return made[-1]

    events = Events()
    recorder = FakeRecorder(DATA / "es.wav", second_wav=DATA / "en.wav")
    session = RecordingSession(model, recorder, RecorderSettings(), tmp_path / "rec", events, refiner=refiner)
    session.start()
    events.wait_for("recording", timeout=15)
    events.wait_for("phrase", timeout=60)
    time.sleep(2.5)
    result = session.stop()

    said = {source: " ".join(s.text for s in result.segments if s.source == source).lower()
            for source in ("mic", "computer")}
    assert "mientras hablo" in said["mic"]  # Parakeet alone hears "mientro"
    assert "live transcription test" in said["computer"]
    assert [refine.language for refine in made] == ["es", "en"]


@pytest.mark.slow
def test_stopping_right_away_still_returns_a_result(model, tmp_path):
    events = Events()
    session = RecordingSession(model, FakeRecorder(DATA / "en.wav"), RecorderSettings(), tmp_path / "rec", events)
    session.start()
    result = session.stop()
    assert events.kinds[-1] == "finished"
    assert result.duration_seconds >= 0


@pytest.mark.slow
@pytest.mark.skipif(os.name != "nt", reason="the audio sources use Windows named pipes")
def test_audio_sources_start_with_the_first_frame_and_stop_with_the_recorder(model, tmp_path):
    events = Events()
    recorder = FakeRecorder(DATA / "en.wav", audio_sources=True)
    session = RecordingSession(model, recorder, RecorderSettings(), tmp_path / "rec", events)
    session.start()
    events.wait_for("recording", timeout=15)
    time.sleep(2)
    stopping = time.monotonic()
    session.stop()
    assert time.monotonic() - stopping < 30
    mic, computer = (int(count) for count in (tmp_path / "rec" / "recording.mp4.audio-bytes").read_text().split())
    assert mic > 1.5 * 48_000 * 4  # more than 1.5 s of 48 kHz mono float32
    assert abs(mic / 4 - computer / 8) < 0.1 * 48_000  # the same length: both start at the same moment
    log = (tmp_path / "rec" / "recorder.log").read_text(encoding="utf-8")
    assert "fake recorder started" in log  # the recorder's own log still lands in the file
    assert "audio start: first frame" in log
    assert "notice" not in events.kinds


@pytest.mark.slow
@pytest.mark.skipif(os.name != "nt", reason="the audio sources use Windows named pipes")
def test_with_a_screen_process_the_audio_starts_at_its_first_frame_and_all_stop_together(model, tmp_path):
    events = Events()
    recorder = FakeRecorder(DATA / "en.wav", audio_sources=True, screen_process=True)
    session = RecordingSession(model, recorder, RecorderSettings(), tmp_path / "rec", events)
    session.start()
    events.wait_for("recording", timeout=15)
    time.sleep(1.5)
    stopping = time.monotonic()
    session.stop()
    assert time.monotonic() - stopping < 30
    assert (tmp_path / "rec" / "finished").exists()  # finish() ran once both had stopped
    log = (tmp_path / "rec" / "recorder.log").read_text(encoding="utf-8")
    assert "[screen] fake screen started" in log and "[audio] [info] fake recorder started" in log
    assert "audio start: first frame" in log
    mic, computer = (int(count) for count in (tmp_path / "rec" / "recording.mp4.audio-bytes").read_text().split())
    assert mic > 1.0 * 48_000 * 4
    assert "ended" not in events.kinds and "notice" not in events.kinds
