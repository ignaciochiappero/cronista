"""The recording's audio on Windows: WASAPI devices, handed to ffmpeg through named pipes.

Each AudioSource captures one WASAPI endpoint, the microphone (a recording device) or the
computer's sound (the default playback device in loopback mode), has Windows convert it to 48 kHz
float32, and writes it into a named pipe that the recorder's ffmpeg opens as an input. FFmpeg has
no WASAPI input, and DirectShow, its way to a microphone, took up to 30 s to open one.

The streams live on the QPC clock (100 ns ticks): the clock WASAPI stamps every packet with, the
one behind time.perf_counter, and the one Windows stamps the screen capture's frames with. All
sources start at the same QPC time, the StartGate's: when the recorder reports the screen's first
frame, which is the video's time zero. So sound and picture start together, and AudioTimeline
keeps them together by placing every packet where its timestamp says.

Loopback hands over nothing at all while nothing plays, and ffmpeg's mix waits for all its inputs,
so AudioTimeline writes silence in the gaps; otherwise the recording's audio would stall.
"""
from __future__ import annotations

import os
import threading
import time
import traceback
import uuid
from typing import Callable

SAMPLE_RATE = 48_000
TICKS_PER_SECOND = 10_000_000  # QPC as WASAPI reports it
POLL_SECONDS = 0.01  # WASAPI delivers 10 ms packets
CHECK_DEVICE_SECONDS = 1.0
FALLBACK_SECONDS = 10.0  # no first frame reported by then: start from when ffmpeg connected
PIPE_ACCESS_OUTBOUND = 0x00000002
PIPE_REJECT_REMOTE_CLIENTS = 0x00000008
ERROR_OPERATION_ABORTED = 995

Notice = Callable[[str], None]


def qpc_now() -> int:
    return time.perf_counter_ns() // 100


class AudioTimeline:
    """Turns packets into one gapless stream that starts at QPC time `start`.

    While sound flows, packets go back to back and the device's clock rules, so timestamp jitter
    changes nothing. A packet goes where its timestamp says after a silence, after a
    discontinuity, or once the device's clock has drifted beyond `slack` (a correction): with
    silence before it, or with its head cut off if that time was already written. idle() writes
    silence while no packet comes, up to `lag` before now, leaving room for packets on their way.
    """

    def __init__(self, start: int, *, rate: int = SAMPLE_RATE, frame_bytes: int = 8,
                 lag_seconds: float = 0.06, slack_seconds: float = 0.02) -> None:
        self.start = start
        self.rate = rate
        self.frame_bytes = frame_bytes
        self.written = 0  # frames handed out so far
        self.corrections = 0
        self._lag = round(lag_seconds * rate)
        self._slack = round(slack_seconds * rate)
        self._flowing = False  # the last frames handed out were a packet's

    def position(self, qpc: int) -> int:
        return (qpc - self.start) * self.rate // TICKS_PER_SECOND

    def packet(self, data: bytes, qpc: int, *, discontinuity: bool = False) -> bytes:
        """What to write for a packet whose first frame is at QPC time `qpc`."""
        frames = len(data) // self.frame_bytes
        target = self.position(qpc)
        if self._flowing:
            if not discontinuity and abs(target - self.written) <= self._slack:
                target = self.written
            elif target != self.written:
                self.corrections += 1
        if target + frames <= self.written:
            return b""  # all of it is already written, or from before the start
        self._flowing = True
        if target >= self.written:
            gap = target - self.written
            self.written = target + frames
            return bytes(gap * self.frame_bytes) + data
        covered = self.written - target
        self.written = target + frames
        return data[covered * self.frame_bytes:]

    def idle(self, now: int) -> bytes:
        """Silence for the time no packet came, up to `lag` before QPC time `now`."""
        target = self.position(now) - self._lag
        if target <= self.written:
            return b""
        gap = target - self.written
        self.written = target
        self._flowing = False
        return bytes(gap * self.frame_bytes)


class StartGate:
    """The QPC time the recording's audio starts at: the same for all its sources.

    The session opens it when the recorder reports the screen's first frame. If that report never
    comes, the first source to wait FALLBACK_SECONDS opens it at the time ffmpeg connected.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.start: int | None = None
        self.how = ""

    def open(self, start: int, how: str) -> int:
        """Sets the start unless it is set already; either way returns the start in force."""
        with self._lock:
            if self.start is None:
                self.start, self.how = start, how
            return self.start


class NamedPipeWriter:
    """The writing end of a named pipe that a reader opens like a file (ffmpeg's -i).

    Overlapped I/O, so that waiting for the reader, or for a reader that stopped taking data,
    can be abandoned when the recording stops.
    """

    def __init__(self, buffer_bytes: int = 1 << 18) -> None:
        import _winapi

        self._api = _winapi
        self.path = rf"\\.\pipe\cronista-{os.getpid()}-{uuid.uuid4().hex[:12]}"
        self._handle = _winapi.CreateNamedPipe(
            self.path,
            PIPE_ACCESS_OUTBOUND | _winapi.FILE_FLAG_OVERLAPPED | _winapi.FILE_FLAG_FIRST_PIPE_INSTANCE,
            _winapi.PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,  # a byte stream, for this machine only
            1, buffer_bytes, 0, 0, _winapi.NULL)

    def wait_for_reader(self, stop: threading.Event) -> bool:
        """Blocks until the reader opens the pipe. False if `stop` was set first."""
        connecting = self._api.ConnectNamedPipe(self._handle, overlapped=True)
        return self._complete(connecting, stop)

    def write(self, data: bytes, stop: threading.Event) -> None:
        """Writes all of `data` unless `stop` is set first. BrokenPipeError: the reader is gone."""
        writing, error = self._api.WriteFile(self._handle, data, overlapped=True)
        if error == self._api.ERROR_IO_PENDING:
            self._complete(writing, stop)
        else:
            writing.GetOverlappedResult(True)

    def _complete(self, operation, stop: threading.Event) -> bool:
        while self._api.WaitForSingleObject(operation.event, 100) != self._api.WAIT_OBJECT_0:
            if stop.is_set():
                operation.cancel()
                break
        _, error = operation.GetOverlappedResult(True)
        return error != ERROR_OPERATION_ABORTED

    def close(self) -> None:
        if self._handle is not None:
            self._api.CloseHandle(self._handle)
            self._handle = None


class AudioSource:
    """One audio input of the recorder: an endpoint, written into a named pipe on the QPC clock.

    Create it before the recorder starts (the pipe must exist when ffmpeg opens it), start() it
    once the recorder runs, and stop() it after the recorder ended. It captures from the moment
    ffmpeg connects but writes nothing until the gate opens; then everything from the gate's start
    on goes out, and what was captured before it is cut.
    """

    def __init__(self, label: str, endpoint, channels: int, gate: StartGate, *, lag_seconds: float,
                 fallback_seconds: float = FALLBACK_SECONDS) -> None:
        self.label = label
        self.channels = channels
        self._frame_bytes = 4 * channels
        self._endpoint = endpoint
        self._gate = gate
        self._lag = lag_seconds
        self._fallback = round(fallback_seconds * TICKS_PER_SECOND)
        self._pipe = NamedPipeWriter()
        self.path = self._pipe.path
        self._timeline: AudioTimeline | None = None
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, name=f"audio: {label}", daemon=True)

    @classmethod
    def microphone(cls, device_id: str, gate: StartGate, on_notice: Notice) -> AudioSource:
        endpoint = WasapiEndpoint(device_id, 1, on_notice, label="The microphone",
                                  missing="The microphone was disconnected, so it is not being recorded. "
                                          "It comes back when it is plugged in again.")
        # A recording device's packets come 15-35 ms after their timestamp, later over Bluetooth:
        # silence only goes in when nothing came for longer than that.
        return cls("microphone", endpoint, 1, gate, lag_seconds=0.2)

    @classmethod
    def computer_sound(cls, gate: StartGate, on_notice: Notice) -> AudioSource:
        endpoint = WasapiEndpoint(None, 2, on_notice, label="The computer's sound",
                                  missing="No speakers or headphones found, so the computer's sound is not "
                                          "being recorded. It starts when one is connected.")
        # Loopback packets carry the time they play, a few milliseconds ahead of their arrival.
        return cls("computer sound", endpoint, 2, gate, lag_seconds=0.06)

    def ffmpeg_input(self) -> list[str]:
        layout = "mono" if self.channels == 1 else "stereo"
        return ["-f", "f32le", "-sample_rate", str(SAMPLE_RATE), "-ch_layout", layout, "-i", self.path]

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread.is_alive():
            self._thread.join(timeout=5)
        self._pipe.close()

    def summary(self) -> str:
        """One line for the recording's log."""
        timeline = self._timeline
        if timeline is None:
            return f"{self.label}: nothing written"
        return f"{self.label}: {timeline.written / SAMPLE_RATE:.1f} s written, {timeline.corrections} timing corrections"

    def _run(self) -> None:
        try:
            if not self._pipe.wait_for_reader(self._stop):
                return
            connected = qpc_now()
            early: list = []  # packets captured before the start was known
            with self._endpoint as endpoint:
                while not self._stop.is_set():
                    packets = endpoint.read()
                    if self._timeline is None:
                        early += packets
                        start = self._gate.start
                        if start is None and qpc_now() - connected > self._fallback:
                            start = self._gate.open(connected, "fallback: no first frame reported")
                        if start is None:
                            self._stop.wait(POLL_SECONDS)
                            continue
                        self._timeline = AudioTimeline(start, frame_bytes=self._frame_bytes, lag_seconds=self._lag)
                        packets, early = early, []
                    for data, qpc, discontinuity in packets:
                        qpc = self._plausible(qpc, len(data))
                        self._write(self._timeline.packet(data, qpc, discontinuity=discontinuity))
                    self._write(self._timeline.idle(qpc_now()))
                    self._stop.wait(POLL_SECONDS)
        except BrokenPipeError:
            pass  # the recorder closed its end of the pipe: it stopped
        except OSError:
            if not self._stop.is_set():
                traceback.print_exc()

    def _plausible(self, qpc: int, size: int) -> int:
        """The packet's timestamp, or its arrival time for a driver without usable timestamps."""
        now = qpc_now()
        if 0 < qpc < now + TICKS_PER_SECOND and now - qpc < 60 * TICKS_PER_SECOND:
            return qpc
        return now - size // self._frame_bytes * TICKS_PER_SECOND // SAMPLE_RATE

    def _write(self, data: bytes) -> None:
        if data:  # a zero-length write can read as the end of the stream
            self._pipe.write(data, self._stop)


class WasapiEndpoint:
    """A WASAPI device read as 48 kHz float32. Nothing here raises.

    device_id None: the default playback device in loopback mode, followed when Windows switches
    it (headphones plugged in, Bluetooth connecting). A device id: that recording device, opened
    again when it comes back after being unplugged. While it fails or is gone, read() hands over
    nothing and the timeline writes silence: a stalled stream would stall the recording's audio.
    """

    def __init__(self, device_id: str | None, channels: int, on_notice: Notice, *, label: str,
                 missing: str) -> None:
        self._device_id = device_id
        self._channels = channels
        self._on_notice = on_notice
        self._label = label
        self._missing = missing
        self._noticed: set[str] = set()
        self._owns_com = False
        self._enumerator = None
        self._open_id: str | None = None
        self._client = None
        self._capture = None
        self._next_check = 0.0

    def __enter__(self) -> WasapiEndpoint:
        try:
            from . import wasapi

            self._wasapi = wasapi
            self._owns_com = wasapi.initialize_thread()
            self._enumerator = wasapi.device_enumerator()
        except Exception as error:  # noqa: BLE001 - e.g. the Windows audio service is not running
            traceback.print_exc()
            self._notice_once("unavailable", f"{self._label} can't be recorded: {error}")
        return self

    def __exit__(self, *exc_info) -> None:
        self._close()
        self._enumerator = None
        if self._owns_com:
            self._wasapi.uninitialize_thread()

    def read(self) -> list[tuple[bytes, int, bool]]:
        if self._enumerator is None:
            return []
        if time.monotonic() >= self._next_check:
            self._next_check = time.monotonic() + CHECK_DEVICE_SECONDS
            self._follow()
        if self._capture is None:
            return []
        try:
            return self._wasapi.read_packets(self._capture, 4 * self._channels)
        except Exception:  # noqa: BLE001 - unplugged or reconfigured; reopened at the next check
            self._close()
            return []

    def _follow(self) -> None:
        target = self._target()
        if target is None:
            self._close()
            self._notice_once("missing", self._missing)
            return
        if target == self._open_id and self._capture is not None:
            return
        self._close()
        try:
            self._client, self._capture = self._wasapi.open_stream(
                self._enumerator, target, SAMPLE_RATE, self._channels, loopback=self._device_id is None)
            self._open_id = target
        except Exception as error:  # noqa: BLE001 - shown once; tried again at every check
            if self._notice_once("open-failed", f"{self._label} can't be recorded: {error}"):
                traceback.print_exc()

    def _target(self) -> str | None:
        """The device to capture now: the default playback device, or the microphone if plugged in."""
        try:
            if self._device_id is None:
                return self._wasapi.default_output_id(self._enumerator)
            return self._device_id if self._wasapi.is_active(self._enumerator, self._device_id) else None
        except Exception:  # noqa: BLE001 - no playback device at all, or an unknown id
            return None

    def _close(self) -> None:
        if self._client is not None:
            try:
                self._client.Stop()
            except Exception:  # noqa: BLE001 - the device may be gone already
                pass
        self._client = self._capture = None
        self._open_id = None

    def _notice_once(self, key: str, message: str) -> bool:
        if key in self._noticed:
            return False
        self._noticed.add(key)
        self._on_notice(message)
        return True
