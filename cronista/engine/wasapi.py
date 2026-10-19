"""The WASAPI calls audio_sources.py needs, through comtypes. Windows only.

pycaw (already used for its device enumerator) defines the enumerator, the device, IAudioClient and
the property store; IAudioCaptureClient and a correctly packed WAVEFORMATEXTENSIBLE are added here.
"""
from __future__ import annotations

import ctypes
from contextlib import contextmanager
from ctypes import POINTER, c_ubyte, c_uint32, c_ulonglong
from ctypes.wintypes import DWORD, WORD

import comtypes
from comtypes import COMMETHOD, GUID, HRESULT, IUnknown
from pycaw.api.audioclient import IAudioClient
from pycaw.api.audioclient.depend import WAVEFORMATEX
from pycaw.api.mmdeviceapi.depend.structures import PROPERTYKEY
from pycaw.pycaw import AudioUtilities

RENDER, CAPTURE = 0, 1  # EDataFlow
CONSOLE = 0  # ERole.eConsole: the "Default Device" in Sound settings
ACTIVE = 0x1  # DEVICE_STATE_ACTIVE
STGM_READ = 0
SHARED = 0
LOOPBACK = 0x00020000
AUTOCONVERTPCM = 0x80000000  # Windows converts to the format asked for: rate, channels, sample type
SRC_DEFAULT_QUALITY = 0x08000000
DATA_DISCONTINUITY = 0x1
SILENT = 0x2
BUFFER_TICKS = 10_000_000  # 1 s in 100 ns units: room for a capture thread that falls behind
WAVE_FORMAT_IEEE_FLOAT = 0x0003
WAVE_FORMAT_EXTENSIBLE = 0xFFFE
SUBTYPE_IEEE_FLOAT = GUID("{00000003-0000-0010-8000-00AA00389B71}")
CHANNEL_MASKS = {1: 0x4, 2: 0x3}  # front center; front left | front right
FRIENDLY_NAME = PROPERTYKEY(GUID("{A45C254E-DF1C-4EFD-8020-67D146A850E0}"), 14)  # PKEY_Device_FriendlyName


class IAudioCaptureClient(IUnknown):
    _iid_ = GUID("{C8ADBD64-E71E-48A0-A4DE-185C395CD317}")
    _methods_ = (
        COMMETHOD([], HRESULT, "GetBuffer",
                  (["out"], POINTER(POINTER(c_ubyte)), "ppData"),
                  (["out"], POINTER(c_uint32), "pNumFramesToRead"),
                  (["out"], POINTER(DWORD), "pdwFlags"),
                  (["out"], POINTER(c_ulonglong), "pu64DevicePosition"),
                  (["out"], POINTER(c_ulonglong), "pu64QPCPosition")),
        COMMETHOD([], HRESULT, "ReleaseBuffer", (["in"], c_uint32, "NumFramesRead")),
        COMMETHOD([], HRESULT, "GetNextPacketSize", (["out"], POINTER(c_uint32), "pNumFramesInNextPacket")),
    )


class WAVEFORMATEXTENSIBLE(ctypes.Structure):
    # Packed as in mmreg.h. pycaw's WAVEFORMATEX is padded to 20 bytes, so it can't be nested here.
    _pack_ = 1
    _fields_ = [
        ("wFormatTag", WORD), ("nChannels", WORD), ("nSamplesPerSec", DWORD), ("nAvgBytesPerSec", DWORD),
        ("nBlockAlign", WORD), ("wBitsPerSample", WORD), ("cbSize", WORD),
        ("wValidBitsPerSample", WORD), ("dwChannelMask", DWORD), ("SubFormat", GUID),
    ]


def float_format(rate: int, channels: int) -> WAVEFORMATEXTENSIBLE:
    fmt = WAVEFORMATEXTENSIBLE()
    fmt.wFormatTag = WAVE_FORMAT_EXTENSIBLE
    fmt.nChannels = channels
    fmt.nSamplesPerSec = rate
    fmt.wBitsPerSample = fmt.wValidBitsPerSample = 32
    fmt.nBlockAlign = 4 * channels
    fmt.nAvgBytesPerSec = rate * 4 * channels
    fmt.cbSize = ctypes.sizeof(WAVEFORMATEXTENSIBLE) - 18
    fmt.dwChannelMask = CHANNEL_MASKS[channels]
    fmt.SubFormat = SUBTYPE_IEEE_FLOAT
    return fmt


def initialize_thread() -> bool:
    """COM for the calling thread. False when the thread already had it in another mode."""
    try:
        comtypes.CoInitializeEx(comtypes.COINIT_MULTITHREADED)
        return True
    except OSError:
        return False


def uninitialize_thread() -> None:
    comtypes.CoUninitialize()


@contextmanager
def com_thread():
    """COM for the calling thread while the block runs (pywebview's threads may not have it).

    COM objects must be gone before it ends, so do the work in a function called inside the block.
    """
    owned = initialize_thread()
    try:
        yield
    finally:
        if owned:
            uninitialize_thread()


def device_enumerator():
    return AudioUtilities.GetDeviceEnumerator()


def capture_devices() -> tuple[list[dict], str | None]:
    """The recording devices that are plugged in, as {"id", "name"}, and the Windows default's id."""
    with com_thread():
        return _capture_devices()


def _capture_devices() -> tuple[list[dict], str | None]:
    enumerator = device_enumerator()
    collection = enumerator.EnumAudioEndpoints(CAPTURE, ACTIVE)
    devices = []
    for index in range(collection.GetCount()):
        device = collection.Item(index)
        devices.append({"id": device.GetId(), "name": _friendly_name(device)})
    try:
        default = enumerator.GetDefaultAudioEndpoint(CAPTURE, CONSOLE).GetId()
    except comtypes.COMError:
        default = None  # no recording device at all
    return devices, default


def _friendly_name(device) -> str:
    value = device.OpenPropertyStore(STGM_READ).GetValue(FRIENDLY_NAME)
    try:
        return value.GetValue() or device.GetId()
    finally:
        value.clear()


def default_output_id(enumerator) -> str:
    """The default playback device's id. Raises COMError when there is no playback device."""
    return enumerator.GetDefaultAudioEndpoint(RENDER, CONSOLE).GetId()


def is_active(enumerator, device_id: str) -> bool:
    """Whether a device is plugged in and enabled. Raises COMError for an id Windows doesn't know."""
    return enumerator.GetDevice(device_id).GetState() == ACTIVE


def open_stream(enumerator, device_id: str, rate: int, channels: int, *, loopback: bool):
    """Starts capturing from a device as `rate` Hz float32 with `channels` channels: (client, capture).

    loopback: what a playback device plays, instead of what a recording device hears.
    """
    device = enumerator.GetDevice(device_id)
    client = _activate(device)
    mode = LOOPBACK if loopback else 0
    wanted = float_format(rate, channels)
    try:
        client.Initialize(SHARED, mode | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY, BUFFER_TICKS, 0,
                          ctypes.cast(ctypes.pointer(wanted), POINTER(WAVEFORMATEX)), None)
    except comtypes.COMError:
        # Some Windows versions may refuse the conversion. Then the device's own mix format must
        # already be the wanted one, as it usually is for playback devices (48 kHz stereo float).
        client = _activate(device)
        mix = client.GetMixFormat()
        try:
            if not _is_float(mix, rate, channels):
                raise RuntimeError(f"the device mixes at {mix.contents.nSamplesPerSec} Hz "
                                   f"with {mix.contents.nChannels} channels") from None
            client.Initialize(SHARED, mode, BUFFER_TICKS, 0, mix, None)
        finally:
            ctypes.windll.ole32.CoTaskMemFree(mix)
    capture = client.GetService(IAudioCaptureClient._iid_).QueryInterface(IAudioCaptureClient)
    client.Start()
    return client, capture


def read_packets(capture, frame_bytes: int) -> list[tuple[bytes, int, bool]]:
    """Every packet waiting: (data, QPC time of its first frame in 100 ns ticks, discontinuity)."""
    packets = []
    while capture.GetNextPacketSize():
        data, frames, flags, _, qpc = capture.GetBuffer()
        size = frames * frame_bytes
        payload = bytes(size) if flags & SILENT else ctypes.string_at(data, size)
        capture.ReleaseBuffer(frames)
        packets.append((payload, qpc, bool(flags & DATA_DISCONTINUITY)))
    return packets


def _activate(device):
    return device.Activate(IAudioClient._iid_, comtypes.CLSCTX_ALL, None).QueryInterface(IAudioClient)


def _is_float(fmt_pointer, rate: int, channels: int) -> bool:
    fmt = fmt_pointer.contents
    if (fmt.nSamplesPerSec, fmt.nChannels, fmt.wBitsPerSample) != (rate, channels, 32):
        return False
    if fmt.wFormatTag == WAVE_FORMAT_EXTENSIBLE:
        return ctypes.cast(fmt_pointer, POINTER(WAVEFORMATEXTENSIBLE)).contents.SubFormat == SUBTYPE_IEEE_FLOAT
    return fmt.wFormatTag == WAVE_FORMAT_IEEE_FLOAT
