"""One recording: the recorder process, live transcription while it runs, and the final pass.

Pulled out of the spike's spike.py. Events reach on_event(kind, payload) from worker threads:

  listening   the recorder started (on Linux the share dialog is up)
  recording   {"started_at"}: the first audio arrived, the recording runs
  partial     {"committed", "pending", "source"}: live text of the phrase in progress
  phrase      {"start", "text", "source"}: a finished phrase, live
  ended       {"code", "reason"}: the recorder stopped on its own (time cap, error); call stop()
  notice      {"message"}: something to tell the user that does not stop the recording
  finalizing  the final pass over the whole recording is running
  finished    the result of stop(), as a dict

The audio for the final pass goes to a file in the recording's folder instead of RAM (an hour is
~230 MB) and is deleted once the final pass is done.

When the recorder streams the microphone and the computer's sound apart (two channels), each one
has its own phrase detection, live text and final pass, and every phrase says which it came from
("source", voices.py); the microphone's echoes of the computer are left out.

When the recorder takes its audio from Python (audio_sources.py, on Windows), the session reads
the recorder's log as it comes: the line about the screen's first frame opens the StartGate, so
the audio starts at the video's time zero.
"""
from __future__ import annotations

import subprocess
import threading
import time
import traceback
from collections import deque
from dataclasses import asdict, dataclass
from datetime import datetime
from pathlib import Path
from typing import Callable

import numpy as np

from .audio_sources import TICKS_PER_SECOND, StartGate, qpc_now
from .final_pass import run_final_pass_file
from .local_agreement import LocalAgreement
from .recorders import IS_WINDOWS, SAMPLE_RATE, RecorderProcess, RecorderSettings
from .transcript import TranscriptionSegment, tidy_text
from .voice_activity import VoiceActivitySegmenter, VoiceEvent
from .voices import SOURCES, drop_echoes, is_echo

READ_BLOCK_FRAMES = 1_600  # 100 ms of each channel
PROBLEM_TAGS = ("[warning]", "[error]", "[fatal]", "[panic]")  # ffmpeg's -loglevel level+... prefixes
EventSink = Callable[[str, dict], None]


class SpeechScheduler:
    """Hands work to the model: every final, but only the newest partial of each channel (from the spike).

    Partials that pile up while the model is busy are dropped in favour of the latest one, so the
    live text never falls behind and a slow machine just updates less often. With two channels, the
    one whose partial has waited longest goes first.
    """

    def __init__(self) -> None:
        self._condition = threading.Condition()
        self._finals: deque[tuple[int, VoiceEvent]] = deque()
        self._partials: dict[int, VoiceEvent] = {}  # in the order they started waiting
        self._closed = False

    def submit(self, channel: int, event: VoiceEvent) -> None:
        with self._condition:
            if event.kind == "final":
                self._finals.append((channel, event))
                self._partials.pop(channel, None)
            else:
                self._partials[channel] = event  # a channel already waiting keeps its place, with newer audio
            self._condition.notify()

    def close(self) -> None:
        with self._condition:
            self._closed = True
            self._condition.notify()

    def next(self) -> tuple[int, VoiceEvent] | None:
        with self._condition:
            while not self._finals and not self._partials and not self._closed:
                self._condition.wait()
            if self._finals:
                return self._finals.popleft()
            if self._partials:
                channel = next(iter(self._partials))
                return channel, self._partials.pop(channel)
            return None


@dataclass
class SessionResult:
    folder: str
    video_path: str
    started_at: str
    duration_seconds: float
    size_bytes: int
    segments: list[TranscriptionSegment]

    @property
    def transcript(self) -> str:
        return " ".join(segment.text for segment in self.segments)

    def to_dict(self) -> dict:
        data = asdict(self)
        data["transcript"] = self.transcript
        return data


class RecordingSession:
    def __init__(self, model, recorder, settings: RecorderSettings, folder: Path, on_event: EventSink) -> None:
        self.model = model
        self.recorder = recorder
        self.settings = settings
        self.folder = Path(folder)
        self.on_event = on_event
        self.tape_paths = [self.folder / "audio-16k.f32"]  # one per channel, see start()
        self.live_segments: list[TranscriptionSegment] = []
        self._channels = 1
        self._segmenters = [VoiceActivitySegmenter(SAMPLE_RATE)]
        self._tapes: list = []
        self._scheduler = SpeechScheduler()
        self._running: list[tuple[RecorderProcess, subprocess.Popen]] = []
        self._speech: subprocess.Popen | None = None  # the process whose stdout feeds the model
        self._gate = StartGate()
        self._sources: list = []
        self._threads: list[threading.Thread] = []
        self._log_readers: list[threading.Thread] = []
        self._samples = 0
        self._started_at: datetime | None = None
        self._stopping = False
        self._ended = False
        self._last_line = self._last_problem = ""
        self._lock = threading.Lock()  # the log readers and stop() share the log file and the processes' stdin

    def start(self) -> None:
        self.folder.mkdir(parents=True, exist_ok=True)
        # The sources' pipes have to exist before the recorder starts and opens them.
        audio_sources = getattr(self.recorder, "audio_sources", None)
        self._sources = audio_sources(self.settings, self._gate, self._notice) if audio_sources else []
        # Out of the console's Ctrl+C reach: only stop() decides when the recorder ends.
        isolation = {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.CREATE_NO_WINDOW} \
            if IS_WINDOWS else {"start_new_session": True}
        try:
            for process in self.recorder.processes(self.folder, self.settings, self._sources):
                popen = subprocess.Popen(process.command, stdin=subprocess.PIPE, stderr=subprocess.PIPE,
                                         stdout=subprocess.PIPE if process.speech else subprocess.DEVNULL, **isolation)
                self._running.append((process, popen))
        except BaseException:
            for _, popen in self._running:
                popen.kill()
                popen.wait()
            for source in self._sources:
                source.stop()
            raise
        speech_process, self._speech = next((process, popen) for process, popen in self._running if process.speech)
        self._channels = speech_process.speech_channels
        if self._channels > 1:
            self.tape_paths = [self.folder / f"audio-16k-{SOURCES[channel]}.f32" for channel in range(self._channels)]
            self._segmenters = [VoiceActivitySegmenter(SAMPLE_RATE) for _ in range(self._channels)]
        for source in self._sources:
            source.start()
        self._log = open(self.folder / "recorder.log", "w", encoding="utf-8")
        self._tapes = [open(path, "wb") for path in self.tape_paths]
        several = len(self._running) > 1
        self._log_readers = [threading.Thread(target=self._read_log, args=(process.name if several else "", popen),
                                              daemon=True) for process, popen in self._running]
        self._threads = [threading.Thread(target=self._read_audio, daemon=True),
                         threading.Thread(target=self._transcribe, daemon=True)]
        for thread in self._log_readers + self._threads:
            thread.start()
        self.on_event("listening", {})

    def stop(self) -> SessionResult:
        self._stopping = True
        # A screen capture that only sends frames when the screen changes gets one now, before "q",
        # so the video lasts until the end; after "q", it needs more of them to notice the request.
        self._wake_screen()
        for _, popen in self._running:
            try:
                self._type(popen, b"q")
            except OSError:
                pass  # already gone
        deadline = time.monotonic() + 30
        for _, popen in self._running:
            while popen.poll() is None:
                self._wake_screen()
                try:
                    popen.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    if time.monotonic() > deadline:
                        popen.kill()
                        popen.wait()
        for source in self._sources:
            source.stop()
        for thread in self._threads:
            thread.join()
        for reader in self._log_readers:
            reader.join(timeout=5)
        if self._sources:
            self._log.write(f"audio start: {self._gate.how or 'never'}\n")
            for source in self._sources:
                self._log.write(source.summary() + "\n")
        self._log.close()

        self.on_event("finalizing", {})
        # Putting the recording's parts together (Windows) runs while the final pass does.
        finishing = threading.Thread(target=self._finish_recording)
        finishing.start()
        segments: list[TranscriptionSegment] = []
        if self._samples:
            for channel, path in enumerate(self.tape_paths):
                segments += [TranscriptionSegment(segment.start, segment.end, segment.text, SOURCES[channel])
                             for segment in run_final_pass_file(self.model, path)]
            segments = drop_echoes(segments)
        for path in self.tape_paths:
            path.unlink(missing_ok=True)
        finishing.join()
        video = self.folder / self.recorder.output_name
        result = SessionResult(
            folder=str(self.folder),
            video_path=str(video),
            started_at=(self._started_at or datetime.now()).isoformat(timespec="seconds"),
            duration_seconds=round(self._samples / SAMPLE_RATE, 1),
            size_bytes=video.stat().st_size if video.exists() else 0,
            segments=segments,
        )
        self.on_event("finished", result.to_dict())
        return result

    def _read_audio(self) -> None:
        frame_bytes = 4 * self._channels
        while True:
            chunk = self._speech.stdout.read(READ_BLOCK_FRAMES * frame_bytes)
            if not chunk:
                break
            if self._started_at is None:
                self._started_at = datetime.now()
                self.on_event("recording", {"started_at": time.time()})
            usable = chunk[: len(chunk) - len(chunk) % frame_bytes]
            frames = np.frombuffer(usable, dtype=np.float32).reshape(-1, self._channels)
            self._samples += len(frames)
            for channel in range(self._channels):
                samples = np.ascontiguousarray(frames[:, channel])
                self._tapes[channel].write(samples.tobytes())
                for event in self._segmenters[channel].push(samples):
                    self._scheduler.submit(channel, event)
        for channel, segmenter in enumerate(self._segmenters):
            for event in segmenter.flush():
                self._scheduler.submit(channel, event)
        for tape in self._tapes:
            tape.close()
        self._scheduler.close()

    def _read_log(self, name: str, popen: subprocess.Popen) -> None:
        """A process's log, into recorder.log (its lines start with its name when there are several).
        The line about the screen's first frame opens the gate; the end of the log is the process's."""
        screen_started = getattr(self.recorder, "screen_started", None)
        frame_report = getattr(self.recorder, "frame_report", None)
        delay = round(getattr(self.recorder, "first_frame_report_delay", 0.0) * TICKS_PER_SECOND)
        prefix = f"[{name}] " if name else ""
        for raw in iter(popen.stderr.readline, b""):
            arrived = qpc_now()
            line = raw.decode("utf-8", "replace").rstrip()
            if self._gate.start is None and screen_started is not None and screen_started(line):
                self._gate.open(arrived - delay, "first frame")
                try:
                    self._type(popen, getattr(self.recorder, "after_first_frame", b""))
                except OSError:
                    pass  # the process is already gone
            elif self._gate.start is not None and frame_report is not None and frame_report(line):
                continue  # the frames after the first, until the process takes the hint
            with self._lock:
                if line:
                    self._last_line = prefix + line
                    if any(tag in line for tag in PROBLEM_TAGS):
                        self._last_problem = prefix + line
                self._log.write(prefix + line + "\n")
        with self._lock:
            self._log.flush()
        self._report_end(popen.wait())

    def _report_end(self, code: int) -> None:
        """A process stopped on its own (time cap, error): say it once, and the app calls stop()."""
        with self._lock:
            if self._stopping or self._ended:
                return
            self._ended = True
        self.on_event("ended", {"code": code, "reason": self._last_problem or self._last_line})

    def _type(self, popen: subprocess.Popen, keys: bytes) -> None:
        """Keys for a process, as if typed in its console (ffmpeg reads them from stdin)."""
        if not keys:
            return
        with self._lock:
            popen.stdin.write(keys)
            popen.stdin.flush()

    def _wake_screen(self) -> None:
        wake = getattr(self.recorder, "wake_screen", None)
        if wake is None:
            return
        try:
            wake()
        except Exception:  # noqa: BLE001 - never in the way of stopping
            traceback.print_exc()

    def _finish_recording(self) -> None:
        finish = getattr(self.recorder, "finish", None)
        if finish is None:
            return
        try:
            finish(self.folder)
        except Exception as error:  # noqa: BLE001 - the parts stay in the folder; the transcript is still saved
            self._notice(str(error))

    def _notice(self, message: str) -> None:
        self.on_event("notice", {"message": message})

    def _transcribe(self) -> None:
        # One agreement per channel steadies its live line; the final pass replaces them anyway.
        agreements = [LocalAgreement() for _ in range(self._channels)]
        heard_from_computer: deque[TranscriptionSegment] = deque(maxlen=20)  # to spot the microphone's echoes
        while (item := self._scheduler.next()) is not None:
            channel, event = item
            source = SOURCES[channel]
            text = tidy_text(str(self.model.recognize(event.audio, sample_rate=SAMPLE_RATE)).strip())
            if event.kind == "partial":
                snapshot = agreements[channel].update(text)
                self.on_event("partial", {"committed": snapshot.committed, "pending": snapshot.pending,
                                          "source": source})
                continue
            agreements[channel].reset()
            if not text:
                continue
            segment = TranscriptionSegment(event.start_sample / SAMPLE_RATE, event.speech_end_sample / SAMPLE_RATE,
                                           text, source)
            if source == "mic" and is_echo(segment, list(heard_from_computer)):
                self.on_event("partial", {"committed": "", "pending": "", "source": source})  # its live line goes
                continue
            if source == "computer":
                heard_from_computer.append(segment)
            self.live_segments.append(segment)
            self.on_event("phrase", {"start": segment.start, "text": text, "source": source})
