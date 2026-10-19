#!/usr/bin/env python3
"""Linux recorder for the spike: the screen through the desktop portal, plus the microphone.

Run it with the system's python3, which has PyGObject (gi) and GStreamer; the speech model runs in
its own environment. The contract with spike.py is the same as ffmpeg's on Windows:
- writes the recording to --out (Matroska, so an interrupted file still plays),
- streams the microphone on stdout as 16 kHz mono float32,
- stops after --seconds of recording, or when it reads "q" on stdin, and closes the file properly,
- writes --status as JSON: the encoder used, frame counts, what the portal answered, errors.

Reused: the GStreamer side follows GNOME's screen-cast.py example
(https://gitlab.gnome.org/-/snippets/15): pipewiresrc on the stream's node id, EOS to finish.
That example talks to Mutter's private org.gnome.Mutter.ScreenCast API, which GNOME now limits to
trusted callers, so the session here goes through org.freedesktop.portal.ScreenCast, in the call
order Chromium/WebRTC also uses: CreateSession, SelectSources, Start, OpenPipeWireRemote.

  python3 portal_recorder.py --check         what this machine can do; records nothing
  python3 portal_recorder.py --test-source   test pattern and tone instead of the portal and mic
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import threading
import time
from dataclasses import dataclass

import gi

gi.require_version("Gst", "1.0")
from gi.repository import Gio, GLib, Gst  # noqa: E402

PORTAL = "org.freedesktop.portal.Desktop"
PORTAL_PATH = "/org/freedesktop/portal/desktop"
SCREENCAST = "org.freedesktop.portal.ScreenCast"
REQUEST = "org.freedesktop.portal.Request"
SESSION = "org.freedesktop.portal.Session"
SOURCE_MONITOR = 1
CURSOR_EMBEDDED = 2
PERSIST_UNTIL_REVOKED = 2
TOKEN_FILE = "/tmp/recorder-spike-restore-token"
FIRST_FRAME_TIMEOUT_S = 8


@dataclass(frozen=True)
class Candidate:
    encoder: str
    memory: str  # "dmabuf": PipeWire may hand over GPU buffers; "system": force a copy to RAM
    chain: str  # from raw video to an encoded stream ready for matroskamux
    needs: tuple[str, ...]


VA_CHAIN = "vapostproc ! video/x-raw(memory:VAMemory),format=NV12 ! {enc} name=enc ! h264parse"
# Hardware first. Each VA encoder is tried twice: with GPU buffers, then forcing system memory in
# case importing the compositor's buffers fails. Software encoders are the last resort.
CANDIDATES = [
    Candidate("vah264lpenc", "dmabuf", VA_CHAIN.format(enc="vah264lpenc"), ("vapostproc", "vah264lpenc", "h264parse")),
    Candidate("vah264enc", "dmabuf", VA_CHAIN.format(enc="vah264enc"), ("vapostproc", "vah264enc", "h264parse")),
    Candidate("vah264lpenc", "system", "video/x-raw ! " + VA_CHAIN.format(enc="vah264lpenc"), ("vapostproc", "vah264lpenc", "h264parse")),
    Candidate("vah264enc", "system", "video/x-raw ! " + VA_CHAIN.format(enc="vah264enc"), ("vapostproc", "vah264enc", "h264parse")),
    Candidate("vaapih264enc", "any", "vaapipostproc ! vaapih264enc name=enc ! h264parse", ("vaapipostproc", "vaapih264enc", "h264parse")),
    Candidate("openh264enc", "system", "videoconvert ! video/x-raw,format=I420 ! openh264enc name=enc ! h264parse", ("videoconvert", "openh264enc", "h264parse")),
    Candidate("x264enc", "system", "videoconvert ! video/x-raw,format=I420 ! x264enc name=enc ! h264parse", ("videoconvert", "x264enc", "h264parse")),
    Candidate("vp8enc", "system", "videoconvert ! vp8enc name=enc", ("videoconvert", "vp8enc")),
]
COMMON_ELEMENTS = ("pipewiresrc", "pulsesrc", "videorate", "identity", "opusenc", "matroskamux", "appsink", "audioresample")

# Applied after the encoder is built; a property the element does not have is skipped and logged,
# so a GStreamer version with different names degrades instead of failing.
ENCODER_TUNING = {
    "vah264lpenc": {"rate-control": "cqp", "qpi": "24", "qpp": "26", "key-int-max": "600"},
    "vah264enc": {"rate-control": "cqp", "qpi": "24", "qpp": "26", "qpb": "28", "key-int-max": "600"},
    "vaapih264enc": {"rate-control": "cqp", "init-qp": "24", "keyframe-period": "600"},
    "openh264enc": {"complexity": "low"},
    "x264enc": {"speed-preset": "veryfast", "pass": "quant", "quantizer": "23", "key-int-max": "600"},
    "vp8enc": {"deadline": "1", "cpu-used": "8", "end-usage": "cq", "cq-level": "10", "threads": "4"},
}
SOURCE_TUNING = {
    "pipewiresrc": {"keepalive-time": "1000"},  # resend the last frame every second on a still screen
    "pulsesrc": {"buffer-time": "40000", "latency-time": "10000"},  # microseconds; low mic latency
}


def log(message: str) -> None:
    sys.stderr.write(f"[{time.strftime('%H:%M:%S')}] {message}\n")
    sys.stderr.flush()


def tune(element: Gst.Element, settings: dict[str, str]) -> None:
    for name, value in settings.items():
        if element.find_property(name) is None:
            log(f"{element.get_factory().get_name()}: no property '{name}', skipped")
            continue
        try:
            Gst.util_set_object_arg(element, name, value)
        except Exception as error:  # noqa: BLE001 - a bad value only loses that setting
            log(f"{element.get_factory().get_name()}: could not set {name}={value}: {error}")


def available(name: str) -> bool:
    return Gst.ElementFactory.find(name) is not None


class PortalError(Exception):
    pass


class ScreenCastPortal:
    """org.freedesktop.portal.ScreenCast over Gio, one synchronous step at a time."""

    def __init__(self) -> None:
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.sender = self.bus.get_unique_name().lstrip(":").replace(".", "_")
        self.session: str | None = None
        self._counter = 0

    def property(self, name: str):
        reply = self.bus.call_sync(
            PORTAL, PORTAL_PATH, "org.freedesktop.DBus.Properties", "Get",
            GLib.Variant("(ss)", (SCREENCAST, name)), GLib.VariantType("(v)"),
            Gio.DBusCallFlags.NONE, 5000, None,
        )
        return reply.unpack()[0]

    def _request(self, method: str, args: tuple, signature: str, options: dict, timeout_s: int = 180) -> dict:
        """Calls a method that answers through a Request object's Response signal."""
        self._counter += 1
        token = f"spike{os.getpid()}_{self._counter}"
        options = dict(options, handle_token=GLib.Variant("s", token))
        expected_path = f"/org/freedesktop/portal/desktop/request/{self.sender}/{token}"
        loop = GLib.MainLoop()
        answer: dict = {}

        def on_response(_conn, _sender, _path, _iface, _signal, params):
            answer["code"], answer["results"] = params.unpack()
            loop.quit()

        # Subscribe before calling, on the path the portal will use, so the answer cannot be missed.
        subscriptions = [self.bus.signal_subscribe(
            PORTAL, REQUEST, "Response", expected_path, None, Gio.DBusSignalFlags.NONE, on_response)]
        try:
            reply = self.bus.call_sync(
                PORTAL, PORTAL_PATH, SCREENCAST, method, GLib.Variant(signature, (*args, options)),
                GLib.VariantType("(o)"), Gio.DBusCallFlags.NONE, -1, None,
            )
            request_path = reply.unpack()[0]
            if request_path != expected_path and "code" not in answer:  # very old portals
                subscriptions.append(self.bus.signal_subscribe(
                    PORTAL, REQUEST, "Response", request_path, None, Gio.DBusSignalFlags.NONE, on_response))
            if "code" not in answer:
                timed_out = []

                def on_timeout():
                    timed_out.append(True)
                    loop.quit()
                    return False

                timer = GLib.timeout_add_seconds(timeout_s, on_timeout)
                loop.run()
                if not timed_out:
                    GLib.source_remove(timer)
        finally:
            for subscription in subscriptions:
                self.bus.signal_unsubscribe(subscription)

        if "code" not in answer:
            raise PortalError(f"{method}: no answer within {timeout_s} s")
        if answer["code"] == 1:
            raise PortalError(f"{method}: cancelled in the dialog")
        if answer["code"] != 0:
            raise PortalError(f"{method}: failed (response code {answer['code']})")
        return answer["results"]

    def create_session(self) -> None:
        results = self._request("CreateSession", (), "(a{sv})",
                                {"session_handle_token": GLib.Variant("s", f"spike{os.getpid()}")})
        self.session = str(results["session_handle"])

    def select_monitor(self, restore_token: str | None) -> None:
        options = {"types": GLib.Variant("u", SOURCE_MONITOR), "multiple": GLib.Variant("b", False)}
        if self.property("AvailableCursorModes") & CURSOR_EMBEDDED:
            options["cursor_mode"] = GLib.Variant("u", CURSOR_EMBEDDED)
        if self.property("version") >= 4:
            options["persist_mode"] = GLib.Variant("u", PERSIST_UNTIL_REVOKED)
            if restore_token:
                options["restore_token"] = GLib.Variant("s", restore_token)
        self._request("SelectSources", (self.session,), "(oa{sv})", options)

    def start(self) -> tuple[int, str | None]:
        results = self._request("Start", (self.session, ""), "(osa{sv})", {})
        streams = results.get("streams") or []
        if not streams:
            raise PortalError("Start returned no streams")
        return int(streams[0][0]), results.get("restore_token")

    def open_pipewire_remote(self) -> int:
        reply, fd_list = self.bus.call_with_unix_fd_list_sync(
            PORTAL, PORTAL_PATH, SCREENCAST, "OpenPipeWireRemote",
            GLib.Variant("(oa{sv})", (self.session, {})), GLib.VariantType("(h)"),
            Gio.DBusCallFlags.NONE, -1, None, None,
        )
        return fd_list.get(reply.unpack()[0])

    def close(self) -> None:
        if not self.session:
            return
        try:
            self.bus.call_sync(PORTAL, self.session, SESSION, "Close", None, None, Gio.DBusCallFlags.NONE, 5000, None)
        except GLib.Error:
            pass
        self.session = None


class Recorder:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.loop = GLib.MainLoop()
        self.portal: ScreenCastPortal | None = None
        self.video_source = ""
        self.pipeline: Gst.Pipeline | None = None
        self.attempt_index = -1
        self.attempt_timer = 0
        self.streaming = False  # the microphone only reaches stdout once a candidate is producing frames
        self.stopping = False
        self.finished = False
        self.recording_started = 0.0
        self.captured = 0
        self.encoded = 0
        self.lock = threading.Lock()
        self.status: dict = {"attempts": [], "errors": [], "portal": {}, "test_source": args.test_source}

    # --- portal -------------------------------------------------------------------------------
    def open_screen(self) -> None:
        if self.args.test_source:
            self.video_source = ("videotestsrc name=src is-live=true pattern=ball ! "
                                 "video/x-raw,width=1920,height=1080,framerate=30/1")
            return
        self.portal = ScreenCastPortal()
        version = self.portal.property("version")
        restore_token = open(TOKEN_FILE).read().strip() if os.path.exists(TOKEN_FILE) else None
        self.status["portal"].update(version=version, restore_token_used=bool(restore_token))
        log(f"portal ScreenCast version {version}; asking to share the screen")
        self.portal.create_session()
        self.portal.select_monitor(restore_token)
        asked = time.monotonic()
        node_id, new_token = self.portal.start()
        self.status["portal"].update(seconds_to_share=round(time.monotonic() - asked, 1), node_id=node_id,
                                     restore_token_returned=bool(new_token))
        if new_token:
            with open(TOKEN_FILE, "w") as token_file:
                token_file.write(new_token)
        self.fd = self.portal.open_pipewire_remote()
        log(f"sharing PipeWire node {node_id}")
        self.video_source = "pipewiresrc name=src do-timestamp=true"

    # --- pipeline -----------------------------------------------------------------------------
    def describe(self, candidate: Candidate) -> str:
        if not self.args.test_source:
            audio_source = "pulsesrc name=mic do-timestamp=true"
        elif self.args.test_audio:
            # A WAV played at real speed (identity sync=true paces it to the clock), so speech goes
            # through the same path a microphone would.
            audio_source = f'filesrc location="{self.args.test_audio}" ! wavparse ! identity name=mic sync=true'
        else:
            audio_source = "audiotestsrc name=mic is-live=true wave=ticks volume=0.3"
        return (
            f"{self.video_source} ! queue max-size-buffers=4 leaky=downstream "
            f"! videorate drop-only=true max-rate={self.args.fps} ! identity name=frames silent=true "
            f"! {candidate.chain} ! queue ! mux. "
            f"{audio_source} ! audioconvert ! audioresample ! audio/x-raw,channels=1,rate=48000 ! tee name=t "
            f"t. ! queue ! opusenc bitrate=96000 ! queue ! mux. "
            f"t. ! queue ! audioconvert ! audioresample "
            f"! audio/x-raw,format=F32LE,rate=16000,channels=1,layout=interleaved "
            f"! appsink name=pcm emit-signals=true sync=false "
            f'matroskamux name=mux ! filesink location="{self.args.out}"'
        )

    def try_next_candidate(self) -> bool:
        while True:
            self.attempt_index += 1
            if self.attempt_index >= len(CANDIDATES):
                self.fail("no encoder produced frames")
                return False
            candidate = CANDIDATES[self.attempt_index]
            if self.args.only and candidate.encoder != self.args.only:
                continue
            missing = [name for name in candidate.needs if not available(name)]
            if missing:
                self.status["attempts"].append({"encoder": candidate.encoder, "memory": candidate.memory,
                                                "result": f"missing {', '.join(missing)}"})
                continue
            if self.start_pipeline(candidate):
                return False  # keep the GLib.idle_add source from repeating

    def start_pipeline(self, candidate: Candidate) -> bool:
        log(f"trying {candidate.encoder} ({candidate.memory} memory)")
        try:
            pipeline = Gst.parse_launch(self.describe(candidate))
        except GLib.Error as error:
            self.status["attempts"].append({"encoder": candidate.encoder, "memory": candidate.memory,
                                            "result": f"pipeline: {error.message}"})
            return False
        src = pipeline.get_by_name("src")
        if not self.args.test_source:
            src.set_property("fd", self.fd)
            node = str(self.status["portal"]["node_id"])
            src.set_property("path" if src.find_property("path") else "target-object", node)
        tune(src, SOURCE_TUNING.get(src.get_factory().get_name(), {}))
        mic = pipeline.get_by_name("mic")
        tune(mic, SOURCE_TUNING.get(mic.get_factory().get_name(), {}))
        tune(pipeline.get_by_name("enc"), ENCODER_TUNING.get(candidate.encoder, {}))

        self.captured = self.encoded = 0
        src.get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, self.on_captured)
        pipeline.get_by_name("enc").get_static_pad("src").add_probe(Gst.PadProbeType.BUFFER, self.on_encoded)
        pipeline.get_by_name("pcm").connect("new-sample", self.on_pcm)
        bus = pipeline.get_bus()
        bus.add_signal_watch()
        bus.connect("message", self.on_message)

        self.pipeline = pipeline
        self.current = candidate
        pipeline.set_state(Gst.State.PLAYING)
        self.attempt_timer = GLib.timeout_add_seconds(FIRST_FRAME_TIMEOUT_S, self.on_attempt_timeout)
        return True

    def drop_pipeline(self) -> None:
        if self.pipeline is not None:
            bus = self.pipeline.get_bus()
            bus.remove_signal_watch()
            self.pipeline.set_state(Gst.State.NULL)
            self.pipeline = None

    def give_up_on_candidate(self, reason: str) -> None:
        log(f"{self.current.encoder} ({self.current.memory}) failed: {reason}")
        self.status["attempts"].append({"encoder": self.current.encoder, "memory": self.current.memory, "result": reason})
        if self.attempt_timer:
            GLib.source_remove(self.attempt_timer)
            self.attempt_timer = 0
        self.drop_pipeline()
        GLib.idle_add(self.try_next_candidate)

    def on_attempt_timeout(self) -> bool:
        self.attempt_timer = 0
        if not self.streaming:
            self.give_up_on_candidate(f"no frames within {FIRST_FRAME_TIMEOUT_S} s")
        return False

    # --- streaming-thread callbacks ------------------------------------------------------------
    def on_captured(self, pad, info):
        with self.lock:
            self.captured += 1
            first = self.captured == 1
        if first:
            caps = pad.get_current_caps()
            self.status["negotiated_caps"] = caps.to_string() if caps else None
        return Gst.PadProbeReturn.OK

    def on_encoded(self, _pad, _info):
        with self.lock:
            self.encoded += 1
            first = self.encoded == 1
        if first:
            GLib.idle_add(self.on_first_frame)
        return Gst.PadProbeReturn.OK

    def on_pcm(self, sink):
        sample = sink.emit("pull-sample")
        if sample is None:
            return Gst.FlowReturn.EOS
        if self.streaming and not self.stopping:
            buffer = sample.get_buffer()
            ok, info = buffer.map(Gst.MapFlags.READ)
            if ok:
                try:
                    sys.stdout.buffer.write(info.data)
                    sys.stdout.buffer.flush()
                except (BrokenPipeError, ValueError):
                    GLib.idle_add(self.request_stop)
                finally:
                    buffer.unmap(info)
        return Gst.FlowReturn.OK

    # --- main-loop callbacks -------------------------------------------------------------------
    def on_first_frame(self) -> bool:
        if self.streaming or self.pipeline is None:
            return False
        if self.attempt_timer:
            GLib.source_remove(self.attempt_timer)
            self.attempt_timer = 0
        self.streaming = True
        self.recording_started = time.monotonic()
        self.status.update(encoder=self.current.encoder, memory=self.current.memory)
        self.status["attempts"].append({"encoder": self.current.encoder, "memory": self.current.memory, "result": "ok"})
        log(f"recording with {self.current.encoder} ({self.current.memory} memory); caps: {self.status.get('negotiated_caps')}")
        GLib.timeout_add_seconds(self.args.seconds, self.request_stop)
        return False

    def on_message(self, _bus, message) -> None:
        if message.type == Gst.MessageType.EOS:
            self.finish()
        elif message.type == Gst.MessageType.ERROR:
            error, debug = message.parse_error()
            text = f"{message.src.get_name()}: {error.message}"
            log(f"error: {text} ({debug})")
            if not self.streaming and not self.stopping:
                self.give_up_on_candidate(text)
            else:
                self.status["errors"].append(text)
                self.finish()
        elif message.type == Gst.MessageType.WARNING:
            warning, _debug = message.parse_warning()
            log(f"warning: {message.src.get_name()}: {warning.message}")

    def request_stop(self) -> bool:
        if self.stopping or self.finished:
            return False
        self.stopping = True
        if self.pipeline is None or not self.streaming:
            self.status["errors"].append("stopped before any frame was recorded")
            self.finish()
            return False
        log("stopping: sending end-of-stream so the file is closed properly")
        self.pipeline.send_event(Gst.Event.new_eos())
        GLib.timeout_add_seconds(10, self.on_eos_timeout)
        return False

    def on_eos_timeout(self) -> bool:
        if not self.finished:
            self.status["errors"].append("end-of-stream did not arrive within 10 s")
            self.finish()
        return False

    def fail(self, reason: str) -> None:
        log(f"giving up: {reason}")
        self.status["errors"].append(reason)
        self.finish()

    def finish(self) -> None:
        if self.finished:
            return
        self.finished = True
        if self.attempt_timer:
            GLib.source_remove(self.attempt_timer)
            self.attempt_timer = 0
        self.drop_pipeline()
        if self.portal is not None:
            self.portal.close()
        self.status.update(captured_frames=self.captured, encoded_frames=self.encoded,
                           recorded_seconds=round(time.monotonic() - self.recording_started, 2) if self.streaming else 0)
        self.write_status()
        self.loop.quit()

    def write_status(self) -> None:
        if self.args.status:
            with open(self.args.status, "w", encoding="utf-8") as status_file:
                json.dump(self.status, status_file, indent=2)

    # --- run ----------------------------------------------------------------------------------
    def watch_stdin(self) -> None:
        # Only "q" stops. A closed stdin is not a stop request: if the parent dies, writing the
        # microphone to stdout fails and on_pcm stops the recording then.
        # os.read, not sys.stdin.buffer.read: a daemon thread blocked inside the buffered reader
        # holds its lock, and Python aborts at exit when it cannot take it (seen as exit code -6).
        while True:
            char = os.read(0, 1)
            if not char:
                log("stdin closed; the recording stops after --seconds")
                return
            if char in b"qQ":
                GLib.idle_add(self.request_stop)
                return

    def run(self) -> int:
        try:
            self.open_screen()
        except (PortalError, GLib.Error) as error:
            message = error.message if isinstance(error, GLib.Error) else str(error)
            self.status["errors"].append(f"portal: {message}")
            log(f"portal: {message}")
            self.write_status()
            return 2
        threading.Thread(target=self.watch_stdin, daemon=True).start()
        GLib.idle_add(self.try_next_candidate)
        self.loop.run()
        return 0 if self.streaming and not self.status["errors"] else 1


def check() -> int:
    report = {
        "session_type": os.environ.get("XDG_SESSION_TYPE"),
        "desktop": os.environ.get("XDG_CURRENT_DESKTOP"),
        "gstreamer": Gst.version_string(),
        "elements": {name: available(name) for name in sorted({n for c in CANDIDATES for n in c.needs} | set(COMMON_ELEMENTS))},
    }
    report["usable_encoders"] = [f"{c.encoder} ({c.memory})" for c in CANDIDATES if all(available(n) for n in c.needs)]
    try:
        portal = ScreenCastPortal()
        report["portal"] = {
            "version": portal.property("version"),
            "monitor_capture": bool(portal.property("AvailableSourceTypes") & SOURCE_MONITOR),
            "cursor_embedded": bool(portal.property("AvailableCursorModes") & CURSOR_EMBEDDED),
        }
    except GLib.Error as error:
        report["portal"] = {"error": error.message}
    print(json.dumps(report, indent=2))
    hardware = [e for e in report["usable_encoders"] if e.startswith("va")]
    ok = (report["elements"].get("pipewiresrc") and report["elements"].get("pulsesrc")
          and report["usable_encoders"] and "error" not in report["portal"])
    log(f"screen capture via portal: {'yes' if 'error' not in report['portal'] else 'NO: ' + report['portal']['error']}")
    log(f"hardware H.264: {', '.join(hardware) if hardware else 'NO (software encoder will be used)'}")
    return 0 if ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", help="recording file (.mkv)")
    parser.add_argument("--seconds", type=int, default=120)
    parser.add_argument("--fps", type=int, default=30)
    parser.add_argument("--status", help="JSON status file written at the end")
    parser.add_argument("--check", action="store_true", help="report capabilities and exit")
    parser.add_argument("--test-source", action="store_true", help="test pattern and tone instead of portal and mic")
    parser.add_argument("--only", help="try only this encoder (for example x264enc), for diagnosis")
    parser.add_argument("--test-audio", help="with --test-source: play this WAV instead of the test tone")
    args = parser.parse_args()
    Gst.init(None)
    if args.check:
        return check()
    if not args.out:
        parser.error("--out is required")
    return Recorder(args).run()


if __name__ == "__main__":
    sys.exit(main())
