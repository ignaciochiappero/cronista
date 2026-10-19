"""Cronista: records the screen and writes down what is said, live and on this machine.

Run from source with: .venv\\Scripts\\python.exe app.py
  --debug       opens the browser developer tools in the window
  --self-test   records 5 seconds (screen, microphone and computer sound) without a window, checks
                the result, deletes it, and exits (used to check a packaged build; the report goes
                to the log file)
"""
import sys
import time
from datetime import datetime
from pathlib import Path

BASE_DIR = Path(getattr(sys, "_MEIPASS", Path(__file__).resolve().parent))


def send_output_to_log() -> None:
    """The packaged app has no console, so its output and errors go to a log file instead.

    Done first, before the engine is imported, so an import error is logged too.
    """
    if not getattr(sys, "frozen", False) and sys.stdout is not None and sys.stderr is not None:
        return
    from cronista.paths import data_dir

    data_dir().mkdir(parents=True, exist_ok=True)
    log = open(data_dir() / "cronista.log", "a", encoding="utf-8", buffering=1)
    log.write(f"\n--- {datetime.now():%Y-%m-%d %H:%M:%S} {' '.join(sys.argv[1:])} ---\n")
    sys.stdout = sys.stderr = log


def self_test() -> int:
    from cronista.app_api import Api

    api = Api()
    notices: list[str] = []  # e.g. the computer's sound could not be recorded
    api._emit = lambda kind, payload: notices.append(payload["message"]) if kind == "notice" else None
    api._model_state = {"state": "loading"}
    api._load_model()
    state = api.state()
    print("self-test: model", state["model"], "| microphones", [m["name"] for m in state["microphones"]],
          "| error", state["recorder_error"] or "none")
    started = api.start(state["settings"]["microphone"], "balanced", True)
    print("self-test: start", started)
    if "error" in started:
        return 1
    # The recorder can take seconds to start (the first run of a freshly built ffmpeg.exe gets an
    # antivirus scan), so the 5 seconds count from the first audio.
    clicked = time.monotonic()
    while api._phase == "listening" and time.monotonic() - clicked < 60:
        time.sleep(0.05)
    print(f"self-test: recording after {time.monotonic() - clicked:.1f} s")
    time.sleep(5)
    api.stop()
    deadline = time.monotonic() + 120
    while api._phase != "idle" and time.monotonic() < deadline:
        time.sleep(0.3)
    records = api.recordings("")
    if not records:
        print("self-test: FAILED, nothing was saved")
        return 1
    record = api.recording(records[0]["id"])
    video = Path(record["video_path"])
    ok = video.exists() and record["size_bytes"] > 0 and record["duration_seconds"] > 4 and not notices
    print(f"self-test: saved {video.name} {record['size_bytes']} bytes, {record['duration_seconds']} s, "
          f"{len(record['segments'])} segments, notices {notices or 'none'} -> {'OK' if ok else 'FAILED'}")
    exported = export_check(record)
    print(f"self-test: exported {exported or 'nothing'}")
    ok = ok and len(exported) == 2
    if ok:
        api.delete(record["id"])
        return 0
    import faulthandler

    print(f"self-test: kept {record['folder']} (recorder.log is there); the threads still running:")
    sys.stdout.flush()
    faulthandler.dump_traceback(file=sys.stdout, all_threads=True)
    return 1


def export_check(record: dict) -> list[str]:
    """Writes the Word and PDF files for a saved recording (without opening Explorer): the files
    that came out non-empty. A silent recording gets a sample line, so the text is laid out too."""
    from datetime import datetime

    from cronista.engine.transcript import TranscriptionSegment
    from cronista.export import write_pdf, write_word

    segments = [TranscriptionSegment(s["start"], s["end"], s["text"]) for s in record["segments"]] \
        or [TranscriptionSegment(0.0, 1.0, "Prueba de exportación: acentos, eñes y ¿signos?")]
    written = []
    for write, name in ((write_word, "transcript.docx"), (write_pdf, "transcript.pdf")):
        path = write(segments, Path(record["folder"]) / name, title=record["title"],
                     recorded_at=datetime.fromisoformat(record["created_at"]))
        if path.stat().st_size > 0:
            written.append(f"{name} ({path.stat().st_size} bytes)")
    return written


def main() -> int:
    send_output_to_log()
    try:
        return run()
    except Exception:  # noqa: BLE001 - logged, since a windowed app has nowhere else to show it
        import traceback

        traceback.print_exc()
        return 1


def run() -> int:
    if "--self-test" in sys.argv:
        return self_test()

    import webview

    from cronista.app_api import Api

    api = Api()
    window = webview.create_window(
        title="Cronista",
        url=str(BASE_DIR / "ui" / "index.html"),
        width=1120,
        height=740,
        min_size=(900, 600),
        js_api=api,
    )
    window.events.closing += api._on_closing
    webview.start(api._attach, (window,), debug="--debug" in sys.argv)
    return 0


if __name__ == "__main__":
    sys.exit(main())
