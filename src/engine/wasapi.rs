//! Native Windows WASAPI device enumeration and audio capture (microphone + system loopback)
//! via official `windows` crate — zero Python / comtypes / pycaw overhead.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

#[cfg(windows)]
mod imp {
    use super::AudioDevice;
    use std::ptr;
    use windows::core::{GUID, HSTRING};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice,
        IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
        AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
        AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    };
    use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
    use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
    use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED, STGM_READ,
    };

    const BUFFER_TICKS: i64 = 10_000_000; // 1 s in 100 ns units
    const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 0x0003;

    pub struct ComGuard {
        owned: bool,
    }

    impl ComGuard {
        pub fn init() -> Self {
            unsafe {
                let hr = CoInitializeEx(None, COINIT_MULTITHREADED);
                Self { owned: hr.is_ok() }
            }
        }
    }

    impl Drop for ComGuard {
        fn drop(&mut self) {
            if self.owned {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn float_format(rate: u32, channels: u16) -> WAVEFORMATEXTENSIBLE {
        let block_align = 4 * channels;
        let mask = if channels == 1 { 0x4 } else { 0x3 };
        WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                nChannels: channels,
                nSamplesPerSec: rate,
                nAvgBytesPerSec: rate * block_align as u32,
                nBlockAlign: block_align,
                wBitsPerSample: 32,
                cbSize: (std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - 18) as u16,
            },
            Samples: windows::Win32::Media::Audio::WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 32,
            },
            dwChannelMask: mask,
            SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
        }
    }

    unsafe fn is_float_format(fmt: *const WAVEFORMATEX, rate: u32, channels: u16) -> bool {
        let f = &*fmt;
        if f.nSamplesPerSec != rate || f.nChannels != channels || f.wBitsPerSample != 32 {
            return false;
        }
        if f.wFormatTag as u32 == WAVE_FORMAT_EXTENSIBLE {
            let ext = &*(fmt as *const WAVEFORMATEXTENSIBLE);
            let sub: GUID = ext.SubFormat;
            return sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
        }
        f.wFormatTag == WAVE_FORMAT_IEEE_FLOAT_TAG
    }

    unsafe fn friendly_name(device: &IMMDevice, fallback_id: &str) -> String {
        let Ok(store) = device.OpenPropertyStore(STGM_READ) else {
            return fallback_id.to_string();
        };
        let Ok(mut prop) = store.GetValue(&PKEY_Device_FriendlyName) else {
            return fallback_id.to_string();
        };
        let name = prop.to_string();
        let _ = PropVariantClear(&mut prop);
        if name.trim().is_empty() {
            fallback_id.to_string()
        } else {
            name
        }
    }

    pub fn device_enumerator() -> Result<IMMDeviceEnumerator, String> {
        unsafe {
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("WASAPI enumerator failed: {e}"))
        }
    }

    pub fn capture_devices() -> Result<(Vec<AudioDevice>, Option<String>), String> {
        let _com = ComGuard::init();
        let enumerator = device_enumerator()?;
        unsafe {
            let collection = enumerator
                .EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)
                .map_err(|e| format!("EnumAudioEndpoints failed: {e}"))?;
            let count = collection.GetCount().unwrap_or(0);
            let mut devices = Vec::with_capacity(count as usize);
            for i in 0..count {
                if let Ok(dev) = collection.Item(i) {
                    if let Ok(id_pwstr) = dev.GetId() {
                        let id = id_pwstr.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(id_pwstr.as_ptr() as *const _));
                        let name = friendly_name(&dev, &id);
                        devices.push(AudioDevice { id, name });
                    }
                }
            }
            let default = enumerator
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .ok()
                .and_then(|dev| {
                    dev.GetId().ok().map(|p| {
                        let s = p.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(p.as_ptr() as *const _));
                        s
                    })
                });
            Ok((devices, default))
        }
    }

    pub fn default_output_id(enumerator: &IMMDeviceEnumerator) -> Result<String, String> {
        unsafe {
            let dev = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|e| format!("No default playback device: {e}"))?;
            let p = dev.GetId().map_err(|e| format!("GetId failed: {e}"))?;
            let id = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.as_ptr() as *const _));
            Ok(id)
        }
    }

    pub fn is_active(enumerator: &IMMDeviceEnumerator, device_id: &str) -> bool {
        unsafe {
            let wide = HSTRING::from(device_id);
            let Ok(dev) = enumerator.GetDevice(&wide) else {
                return false;
            };
            dev.GetState().map(|s| s == DEVICE_STATE_ACTIVE).unwrap_or(false)
        }
    }

    pub struct WasapiStream {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        frame_bytes: usize,
    }

    impl WasapiStream {
        pub fn open(
            enumerator: &IMMDeviceEnumerator,
            device_id: &str,
            rate: u32,
            channels: u16,
            loopback: bool,
        ) -> Result<Self, String> {
            unsafe {
                let wide = HSTRING::from(device_id);
                let device = enumerator
                    .GetDevice(&wide)
                    .map_err(|e| format!("GetDevice failed: {e}"))?;
                let mut client: IAudioClient = device
                    .Activate(CLSCTX_ALL, None)
                    .map_err(|e| format!("Activate IAudioClient failed: {e}"))?;
                let mode = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
                let wanted = float_format(rate, channels);

                let init_res = client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    mode | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    BUFFER_TICKS,
                    0,
                    &wanted.Format as *const WAVEFORMATEX,
                    None,
                );

                if init_res.is_err() {
                    client = device
                        .Activate(CLSCTX_ALL, None)
                        .map_err(|e| format!("Activate fallback failed: {e}"))?;
                    let mix = client
                        .GetMixFormat()
                        .map_err(|e| format!("GetMixFormat failed: {e}"))?;
                    let ok = is_float_format(mix, rate, channels);
                    let mix_rate = (*mix).nSamplesPerSec;
                    let mix_ch = (*mix).nChannels;
                    let res = if ok {
                        client.Initialize(
                            AUDCLNT_SHAREMODE_SHARED,
                            mode,
                            BUFFER_TICKS,
                            0,
                            mix,
                            None,
                        )
                    } else {
                        CoTaskMemFree(Some(mix as *const _));
                        return Err(format!(
                            "the device mixes at {mix_rate} Hz with {mix_ch} channels"
                        ));
                    };
                    CoTaskMemFree(Some(mix as *const _));
                    res.map_err(|e| format!("WASAPI fallback Initialize failed: {e}"))?;
                }

                let capture: IAudioCaptureClient = client
                    .GetService()
                    .map_err(|e| format!("GetService IAudioCaptureClient failed: {e}"))?;
                client
                    .Start()
                    .map_err(|e| format!("IAudioClient::Start failed: {e}"))?;

                Ok(Self {
                    client,
                    capture,
                    frame_bytes: 4 * channels as usize,
                })
            }
        }

        pub fn read_packets(&self) -> Result<Vec<(Vec<u8>, i64, bool)>, String> {
            let mut packets = Vec::new();
            unsafe {
                loop {
                    let next = self
                        .capture
                        .GetNextPacketSize()
                        .map_err(|e| format!("GetNextPacketSize failed: {e}"))?;
                    if next == 0 {
                        break;
                    }
                    let mut data_ptr: *mut u8 = ptr::null_mut();
                    let mut num_frames: u32 = 0;
                    let mut flags: u32 = 0;
                    let mut dev_pos: u64 = 0;
                    let mut qpc_pos: u64 = 0;
                    self.capture
                        .GetBuffer(
                            &mut data_ptr,
                            &mut num_frames,
                            &mut flags,
                            Some(&mut dev_pos),
                            Some(&mut qpc_pos),
                        )
                        .map_err(|e| format!("GetBuffer failed: {e}"))?;

                    let size = num_frames as usize * self.frame_bytes;
                    let payload = if (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data_ptr.is_null() {
                        vec![0u8; size]
                    } else {
                        std::slice::from_raw_parts(data_ptr, size).to_vec()
                    };
                    let _ = self.capture.ReleaseBuffer(num_frames);
                    let discontinuity = (flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;
                    packets.push((payload, qpc_pos as i64, discontinuity));
                }
            }
            Ok(packets)
        }
    }

    impl Drop for WasapiStream {
        fn drop(&mut self) {
            unsafe {
                let _ = self.client.Stop();
            }
        }
    }
}

#[cfg(windows)]
pub use imp::{capture_devices, default_output_id, device_enumerator, is_active, ComGuard, WasapiStream};

#[cfg(not(windows))]
pub fn capture_devices() -> Result<(Vec<AudioDevice>, Option<String>), String> {
    Ok((
        vec![AudioDevice {
            id: "default".to_string(),
            name: "Default microphone".to_string(),
        }],
        Some("default".to_string()),
    ))
}
