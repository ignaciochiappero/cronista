import os
import threading
import time

import pytest
from fakes import SilentEndpoint

from cronista.engine.audio_sources import SAMPLE_RATE, TICKS_PER_SECOND, AudioTimeline, StartGate, qpc_now

windows_only = pytest.mark.skipif(os.name != "nt", reason="WASAPI and named pipes are Windows only")

# One frame per millisecond and one byte per frame keep the arithmetic readable:
# 10 frames = 10 ms = 10 bytes. Sound is b"\x01", silence is b"\x00".
RATE = 1_000
START = 5_000 * TICKS_PER_SECOND  # QPC at stream position 0, in 100 ns ticks


def at(ms: int) -> int:
    return START + ms * (TICKS_PER_SECOND // RATE)


def sound(ms: int) -> bytes:
    return b"\x01" * ms


def silence(ms: int) -> bytes:
    return b"\x00" * ms


def timeline() -> AudioTimeline:
    return AudioTimeline(START, rate=RATE, frame_bytes=1, lag_seconds=0.06, slack_seconds=0.2)


def test_back_to_back_packets_pass_through_unchanged():
    line = timeline()
    assert line.packet(sound(10), at(0)) == sound(10)
    assert line.packet(sound(10), at(10)) == sound(10)
    assert line.written == 20


def test_jitter_in_the_timestamps_adds_no_gaps_and_cuts_nothing():
    line = timeline()
    assert line.packet(sound(10), at(0)) == sound(10)
    assert line.packet(sound(10), at(13)) == sound(10)  # stamped 3 ms late
    assert line.packet(sound(10), at(18)) == sound(10)  # and 2 ms early
    assert line.written == 30
    assert line.corrections == 0


def test_silence_fills_the_time_when_nothing_plays():
    line = timeline()
    assert line.idle(at(100)) == silence(40)  # up to 60 ms before now: a packet may still be on its way
    assert line.idle(at(150)) == silence(50)
    assert line.written == 90


def test_idle_adds_nothing_while_packets_keep_coming():
    line = timeline()
    line.packet(sound(10), at(0))
    line.packet(sound(10), at(10))
    assert line.idle(at(30)) == b""


def test_sound_after_a_silence_lands_at_its_timestamp():
    line = timeline()
    line.idle(at(100))
    assert line.packet(sound(10), at(95)) == silence(55) + sound(10)
    assert line.written == 105
    assert line.corrections == 0  # placing sound after a silence is not a correction


def test_a_late_packet_loses_only_what_silence_already_covered():
    line = timeline()
    line.idle(at(100))  # silence up to 40 ms
    assert line.packet(sound(10), at(35)) == sound(5)
    assert line.written == 45


def test_audio_from_before_the_start_is_dropped():
    line = timeline()
    assert line.packet(sound(10), at(-4)) == sound(6)
    assert line.written == 6


def test_packets_from_before_the_start_change_nothing():
    # A source captures from the moment ffmpeg connects; the start comes later, with the first frame.
    line = timeline()
    assert line.packet(sound(10), at(-30)) == b""
    assert line.packet(sound(10), at(-20)) == b""
    assert line.packet(sound(10), at(-5)) == sound(5)
    assert (line.written, line.corrections) == (5, 0)


def test_a_discontinuity_places_the_packet_by_its_timestamp():
    line = timeline()
    line.packet(sound(10), at(0))
    assert line.packet(sound(10), at(50), discontinuity=True) == silence(40) + sound(10)
    assert line.written == 60


def test_drift_beyond_the_slack_is_corrected_and_counted():
    line = timeline()
    line.packet(sound(10), at(0))
    assert line.packet(sound(10), at(300)) == silence(290) + sound(10)
    assert line.corrections == 1


def test_the_first_to_open_the_gate_sets_the_start_for_everyone():
    gate = StartGate()
    assert gate.start is None
    assert gate.open(100, "first frame") == 100
    assert gate.open(50, "fallback") == 100
    assert (gate.start, gate.how) == (100, "first frame")


# --- sources through the named pipe, on Windows ---------------------------------------------------


class PipeReader(threading.Thread):
    """Reads a source's pipe the way ffmpeg does, until the source closes it."""

    def __init__(self, path: str) -> None:
        super().__init__(daemon=True)
        self.path = path
        self.data = bytearray()

    def run(self) -> None:
        with open(self.path, "rb", buffering=0) as pipe:
            while chunk := pipe.read(65536):
                self.data += chunk


class OnePacketEndpoint(SilentEndpoint):
    """Hands over 0.2 s of sound on the first read, stamped as captured 0.1 s before."""

    def __init__(self) -> None:
        self.stamp: int | None = None

    def read(self) -> list:
        if self.stamp is not None:
            return []
        self.stamp = qpc_now() - TICKS_PER_SECOND // 10
        return [(b"\x01" * 4 * (SAMPLE_RATE // 5), self.stamp, False)]


def seconds_of(data: bytes, channels: int) -> float:
    return len(data) / (SAMPLE_RATE * 4 * channels)


def run_for(gate: StartGate, source, seconds: float) -> float:
    """Opens the gate now, stops the source after `seconds`: the real time between the two."""
    opened = qpc_now()
    gate.open(opened, "test")
    time.sleep(seconds)
    source.stop()
    return (qpc_now() - opened) / TICKS_PER_SECOND


@windows_only
def test_a_source_writes_nothing_until_the_start_is_known():
    from cronista.engine.audio_sources import AudioSource

    gate = StartGate()
    source = AudioSource("test", SilentEndpoint(), 1, gate, lag_seconds=0.06)
    reader = PipeReader(source.path)
    reader.start()
    source.start()
    time.sleep(0.5)
    assert len(reader.data) == 0
    gate.open(qpc_now(), "test")
    time.sleep(1.0)
    source.stop()
    reader.join(timeout=5)
    assert 0.8 < seconds_of(reader.data, 1) < 1.05


@windows_only
def test_without_a_first_frame_the_audio_starts_from_the_connection():
    from cronista.engine.audio_sources import AudioSource

    gate = StartGate()
    source = AudioSource("test", SilentEndpoint(), 1, gate, lag_seconds=0.06, fallback_seconds=0.3)
    reader = PipeReader(source.path)
    reader.start()
    source.start()
    time.sleep(1.0)
    source.stop()
    reader.join(timeout=5)
    assert gate.how.startswith("fallback")
    assert 0.8 < seconds_of(reader.data, 1) < 1.05


@windows_only
def test_audio_captured_before_the_start_is_cut():
    from cronista.engine.audio_sources import AudioSource

    gate = StartGate()
    endpoint = OnePacketEndpoint()
    source = AudioSource("test", endpoint, 1, gate, lag_seconds=0.06)
    reader = PipeReader(source.path)
    reader.start()
    source.start()
    deadline = time.monotonic() + 5
    while endpoint.stamp is None and time.monotonic() < deadline:
        time.sleep(0.01)
    gate.open(endpoint.stamp + TICKS_PER_SECOND * 15 // 100, "test")  # 0.15 s into the packet
    time.sleep(0.5)
    source.stop()
    reader.join(timeout=5)
    sound_bytes = len(reader.data) - len(reader.data.lstrip(b"\x01"))
    assert sound_bytes == 4 * SAMPLE_RATE // 20  # the last 0.05 s of the packet, then silence
    assert set(reader.data[sound_bytes:]) <= {0}


@windows_only
def test_the_computer_sound_streams_in_real_time_even_when_nothing_plays():
    from cronista.engine.audio_sources import AudioSource

    notices: list[str] = []
    gate = StartGate()
    source = AudioSource.computer_sound(gate, notices.append)
    reader = PipeReader(source.path)
    reader.start()
    source.start()
    elapsed = run_for(gate, source, 1.5)
    reader.join(timeout=5)
    # Real time from the start to the stop, minus the 60 ms lag and a poll or two; never ahead of it.
    assert elapsed - 0.25 < seconds_of(reader.data, 2) <= elapsed + 0.01, f"{elapsed:.2f} s elapsed"
    assert len(reader.data) % 8 == 0
    assert not notices


@windows_only
def test_the_microphone_streams_in_real_time():
    from cronista.engine import wasapi
    from cronista.engine.audio_sources import AudioSource

    devices, default = wasapi.capture_devices()
    if not devices:
        pytest.skip("no microphone on this machine")
    notices: list[str] = []
    gate = StartGate()
    source = AudioSource.microphone(default or devices[0]["id"], gate, notices.append)
    reader = PipeReader(source.path)
    reader.start()
    source.start()
    elapsed = run_for(gate, source, 1.5)
    reader.join(timeout=5)
    # Packets come 15-35 ms after their stamp; the last ones may not have come before the stop.
    assert elapsed - 0.3 < seconds_of(reader.data, 1) <= elapsed + 0.01, f"{elapsed:.2f} s elapsed"
    assert not notices


@windows_only
def test_microphones_are_listed_with_their_names_and_the_default():
    from cronista.engine import wasapi

    devices, default = wasapi.capture_devices()
    if not devices:
        pytest.skip("no microphone on this machine")
    assert all(device["id"] and device["name"] for device in devices)
    assert default is None or default in {device["id"] for device in devices}


# --- the named pipe itself ------------------------------------------------------------------------


def read_pipe(path: str, received: bytearray) -> None:
    with open(path, "rb", buffering=0) as pipe:
        while chunk := pipe.read(65536):
            received += chunk


@windows_only
def test_the_pipe_delivers_everything_to_a_reader_that_opens_it_as_a_file():
    from cronista.engine.audio_sources import NamedPipeWriter

    writer = NamedPipeWriter()
    received = bytearray()
    reader = threading.Thread(target=read_pipe, args=(writer.path, received))
    reader.start()
    stop = threading.Event()
    assert writer.wait_for_reader(stop)
    payload = bytes(range(256)) * 4096  # 1 MB, more than the pipe buffers
    writer.write(payload, stop)
    writer.close()
    reader.join(timeout=10)
    assert bytes(received) == payload


@windows_only
def test_waiting_for_a_reader_gives_up_when_stopped():
    from cronista.engine.audio_sources import NamedPipeWriter

    writer = NamedPipeWriter()
    stop = threading.Event()
    threading.Timer(0.2, stop.set).start()
    started = time.monotonic()
    assert not writer.wait_for_reader(stop)
    assert time.monotonic() - started < 2
    writer.close()


@windows_only
def test_writing_after_the_reader_left_raises_broken_pipe():
    from cronista.engine.audio_sources import NamedPipeWriter

    writer = NamedPipeWriter()
    stop = threading.Event()
    reader = threading.Thread(target=lambda: open(writer.path, "rb").close())
    reader.start()
    reader.join()
    with pytest.raises(BrokenPipeError):
        writer.wait_for_reader(stop)  # raises already if the reader came and went before it
        for _ in range(100):
            writer.write(b"\x00" * 4096, stop)
    writer.close()
