"""Downloads a model's files the first time the user asks for that model.

Each file is written as "<name>.part" and only becomes "<name>" once its size and SHA-256 are the ones
written in the code that asks for it, so a file that is there is the expected one. A download that
stops (the user cancels, the connection drops) keeps its part and goes on from there next time, with an
HTTP range request; a part that turns out wrong is deleted. Only the standard library is used.
"""
from __future__ import annotations

import hashlib
import http.client
import urllib.error
import urllib.request
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

CHUNK = 1 << 20  # bytes read at a time; progress is told after each


@dataclass(frozen=True)
class RemoteFile:
    name: str
    url: str
    size: int
    sha256: str


class DownloadFailed(Exception):
    """The download could not be made; the message is for the user."""


class DownloadCancelled(Exception):
    """stopped() said so; what arrived is kept for the next try."""


def missing(folder: Path, files: list[RemoteFile]) -> list[RemoteFile]:
    """The files not in `folder` yet, or not of their size."""
    return [file for file in files
            if not ((Path(folder) / file.name).is_file() and (Path(folder) / file.name).stat().st_size == file.size)]


def _why(error: Exception) -> str:
    """The short reason: "HTTP 404", "[Errno 11001] getaddrinfo failed" rather than urllib's wrapping of it."""
    if isinstance(error, urllib.error.HTTPError):
        return f"HTTP {error.code}"
    if isinstance(error, urllib.error.URLError):
        return str(error.reason)
    return str(error) or type(error).__name__


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        while block := source.read(CHUNK):
            digest.update(block)
    return digest.hexdigest()


def download(folder: Path, files: list[RemoteFile], *, progress: Callable[[int, int], None],
             stopped: Callable[[], bool] = lambda: False, timeout: float = 30) -> None:
    """Downloads the `files` that are missing into `folder`, telling progress(bytes there, bytes in all)."""
    folder = Path(folder)
    folder.mkdir(parents=True, exist_ok=True)
    total = sum(file.size for file in files)
    todo = missing(folder, files)
    done = total - sum(file.size for file in todo)
    for file in todo:
        part = folder / f"{file.name}.part"
        have = part.stat().st_size if part.exists() else 0
        if have > file.size:
            have = 0
        headers = {"User-Agent": "Cronista", **({"Range": f"bytes={have}-"} if have else {})}
        try:
            with urllib.request.urlopen(urllib.request.Request(file.url, headers=headers), timeout=timeout) as response:
                if have and response.status != 206:  # the server sent the whole file again
                    have = 0
                with open(part, "ab" if have else "wb") as out:
                    while block := response.read(CHUNK):
                        out.write(block)
                        have += len(block)
                        progress(done + have, total)
                        if stopped():
                            raise DownloadCancelled
        except (urllib.error.URLError, http.client.HTTPException, OSError) as error:
            raise DownloadFailed(f"Downloading the model failed ({_why(error)}). Check the connection and try again.") \
                from error
        if have < file.size:  # the connection ended early: what arrived stays for next time
            raise DownloadFailed("Downloading the model failed (the connection ended early). "
                                 "Check the connection and try again.")
        if have > file.size or _sha256(part) != file.sha256:
            part.unlink(missing_ok=True)
            raise DownloadFailed("The downloaded model is not the expected one. Try again.")
        part.replace(folder / file.name)
        done += file.size
    progress(total, total)
