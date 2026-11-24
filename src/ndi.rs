//! The NDI® SDK (`libndi.so.6`) loaded at run time, the way the SDK
//! documents it ("Dynamic Loading"): `dlopen` the library from
//! `$NDI_RUNTIME_DIR_V6` (else the linker path), then `NDIlib_v5_load()`
//! returns a table of every NDI function. So the binary, its tests and its
//! tools run without the SDK, and the SDK is never linked in.
//!
//! The types below follow the SDK headers (`Processing.NDI.*.h`, MIT
//! licensed as the SDK says of its headers); only what the bridge uses is
//! typed. NDI® is a registered trademark of Vizrt NDI AB (https://ndi.video/).

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use libloading::Library;

/// The SONAME the SDK's headers name (`NDILIB_LIBRARY_NAME` on Linux).
pub const LIBRARY: &str = "libndi.so.6";
/// Environment variable with the runtime's folder (`NDILIB_REDIST_FOLDER`).
pub const RUNTIME_DIR_VAR: &str = "NDI_RUNTIME_DIR_V6";

type Handle = *mut c_void;

/// `NDIlib_frame_type_e`.
pub mod frame_type {
    pub const NONE: i32 = 0;
    pub const VIDEO: i32 = 1;
    pub const AUDIO: i32 = 2;
    pub const METADATA: i32 = 3;
    pub const ERROR: i32 = 4;
    pub const STATUS_CHANGE: i32 = 100;
    pub const SOURCE_CHANGE: i32 = 101;
}

/// FourCCs (little endian, `NDI_LIB_FOURCC`).
pub mod fourcc {
    const fn cc(s: &[u8; 4]) -> i32 {
        (s[0] as u32 | (s[1] as u32) << 8 | (s[2] as u32) << 16 | (s[3] as u32) << 24) as i32
    }
    pub const UYVY: i32 = cc(b"UYVY");
    pub const UYVA: i32 = cc(b"UYVA");
    pub const P216: i32 = cc(b"P216");
    pub const PA16: i32 = cc(b"PA16");
    pub const BGRA: i32 = cc(b"BGRA");
    pub const BGRX: i32 = cc(b"BGRX");
    pub const NV12: i32 = cc(b"NV12");
    pub const I420: i32 = cc(b"I420");
    pub const FLTP: i32 = cc(b"FLTp");

    pub fn name(c: i32) -> String {
        let b = c.to_le_bytes();
        if b.iter().all(|x| x.is_ascii_alphanumeric()) {
            String::from_utf8_lossy(&b).into_owned()
        } else {
            format!("0x{c:08x}")
        }
    }
}

/// `NDIlib_frame_format_type_e`.
pub mod frame_format {
    pub const INTERLEAVED: i32 = 0;
    pub const PROGRESSIVE: i32 = 1;
    pub const FIELD_0: i32 = 2;
    pub const FIELD_1: i32 = 3;
}

/// `NDIlib_recv_color_format_e` (the ones the bridge offers).
pub mod color_format {
    /// UYVY, UYVA with alpha
    pub const FASTEST: i32 = 100;
    /// Closest to the codec: P216 for NDI's own (SpeedHQ) codec, UYVY for
    /// 8-bit codecs (H.264); PA16 / UYVA with alpha
    pub const BEST: i32 = 101;
}

/// `NDIlib_recv_bandwidth_e`.
pub mod bandwidth {
    pub const LOWEST: i32 = 0;
    pub const HIGHEST: i32 = 100;
}

/// `NDIlib_send_timecode_synthesize` / `NDIlib_recv_timestamp_undefined`.
pub const TIMECODE_SYNTHESIZE: i64 = i64::MAX;
pub const TIMESTAMP_UNDEFINED: i64 = i64::MAX;

/// `NDIlib_source_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Source {
    pub p_ndi_name: *const c_char,
    pub p_url_address: *const c_char,
}

/// `NDIlib_video_frame_v2_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct VideoFrame {
    pub xres: c_int,
    pub yres: c_int,
    pub fourcc: c_int,
    pub frame_rate_n: c_int,
    pub frame_rate_d: c_int,
    pub picture_aspect_ratio: f32,
    pub frame_format_type: c_int,
    /// 100 ns units, set by the sender (or synthesised by the SDK)
    pub timecode: i64,
    pub p_data: *mut u8,
    pub line_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    /// 100 ns UTC since 1970, set by the sending SDK when the frame was
    /// submitted (`TIMESTAMP_UNDEFINED` if not)
    pub timestamp: i64,
}

impl Default for VideoFrame {
    fn default() -> Self {
        // SAFETY: plain data; all zero is "no frame".
        unsafe { std::mem::zeroed() }
    }
}

/// `NDIlib_audio_frame_v3_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AudioFrame {
    pub sample_rate: c_int,
    pub no_channels: c_int,
    pub no_samples: c_int,
    pub timecode: i64,
    pub fourcc: c_int,
    pub p_data: *mut u8,
    /// FLTP: bytes between channels
    pub channel_stride_in_bytes: c_int,
    pub p_metadata: *const c_char,
    pub timestamp: i64,
}

impl Default for AudioFrame {
    fn default() -> Self {
        // SAFETY: plain data.
        unsafe { std::mem::zeroed() }
    }
}

/// `NDIlib_metadata_frame_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MetadataFrame {
    pub length: c_int,
    pub timecode: i64,
    pub p_data: *mut c_char,
}

impl Default for MetadataFrame {
    fn default() -> Self {
        // SAFETY: plain data.
        unsafe { std::mem::zeroed() }
    }
}

/// `NDIlib_recv_create_v3_t`.
#[repr(C)]
pub struct RecvCreate {
    pub source_to_connect_to: Source,
    pub color_format: c_int,
    pub bandwidth: c_int,
    pub allow_video_fields: bool,
    pub p_ndi_recv_name: *const c_char,
}

/// `NDIlib_send_create_t`.
#[repr(C)]
pub struct SendCreate {
    pub p_ndi_name: *const c_char,
    pub p_groups: *const c_char,
    pub clock_video: bool,
    pub clock_audio: bool,
}

/// `NDIlib_find_create_t`.
#[repr(C)]
pub struct FindCreate {
    pub show_local_sources: bool,
    pub p_groups: *const c_char,
    pub p_extra_ips: *const c_char,
}

/// `NDIlib_recv_performance_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct Performance {
    pub video_frames: i64,
    pub audio_frames: i64,
    pub metadata_frames: i64,
}

/// `NDIlib_recv_queue_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct Queue {
    pub video_frames: c_int,
    pub audio_frames: c_int,
    pub metadata_frames: c_int,
}

/// `NDIlib_v5` (= the first 107 slots of `NDIlib_v6_3`, generated from
/// `Processing.NDI.DynamicLoad.h` of NDI SDK 6.4): one pointer per slot,
/// unused ones opaque.
#[repr(C)]
#[allow(non_snake_case)]
pub struct Lib {
    /// slot 0
    pub initialize: unsafe extern "C" fn() -> bool,
    /// slot 1
    pub destroy: unsafe extern "C" fn(),
    /// slot 2
    pub version: unsafe extern "C" fn() -> *const c_char,
    /// slot 3
    pub is_supported_CPU: unsafe extern "C" fn() -> bool,
    _4_find_create: *const c_void,
    /// slot 5
    pub find_create_v2: unsafe extern "C" fn(*const FindCreate) -> Handle,
    /// slot 6
    pub find_destroy: unsafe extern "C" fn(Handle),
    _7_find_get_sources: *const c_void,
    /// slot 8
    pub send_create: unsafe extern "C" fn(*const SendCreate) -> Handle,
    /// slot 9
    pub send_destroy: unsafe extern "C" fn(Handle),
    _10_send_send_video: *const c_void,
    _11_send_send_video_async: *const c_void,
    _12_send_send_audio: *const c_void,
    _13_send_send_metadata: *const c_void,
    _14_send_capture: *const c_void,
    _15_send_free_metadata: *const c_void,
    _16_send_get_tally: *const c_void,
    /// slot 17
    pub send_get_no_connections: unsafe extern "C" fn(Handle, u32) -> c_int,
    _18_send_clear_connection_metadata: *const c_void,
    _19_send_add_connection_metadata: *const c_void,
    _20_send_set_failover: *const c_void,
    _21_recv_create_v2: *const c_void,
    _22_recv_create: *const c_void,
    /// slot 23
    pub recv_destroy: unsafe extern "C" fn(Handle),
    _24_recv_capture: *const c_void,
    _25_recv_free_video: *const c_void,
    _26_recv_free_audio: *const c_void,
    /// slot 27
    pub recv_free_metadata: unsafe extern "C" fn(Handle, *const MetadataFrame),
    _28_recv_send_metadata: *const c_void,
    _29_recv_set_tally: *const c_void,
    /// slot 30
    pub recv_get_performance: unsafe extern "C" fn(Handle, *mut Performance, *mut Performance),
    /// slot 31
    pub recv_get_queue: unsafe extern "C" fn(Handle, *mut Queue),
    _32_recv_clear_connection_metadata: *const c_void,
    _33_recv_add_connection_metadata: *const c_void,
    /// slot 34
    pub recv_get_no_connections: unsafe extern "C" fn(Handle) -> c_int,
    _35_routing_create: *const c_void,
    _36_routing_destroy: *const c_void,
    _37_routing_change: *const c_void,
    _38_routing_clear: *const c_void,
    _39_util_send_send_audio_interleaved_16s: *const c_void,
    _40_util_audio_to_interleaved_16s: *const c_void,
    _41_util_audio_from_interleaved_16s: *const c_void,
    /// slot 42
    pub find_wait_for_sources: unsafe extern "C" fn(Handle, u32) -> bool,
    /// slot 43
    pub find_get_current_sources: unsafe extern "C" fn(Handle, *mut u32) -> *const Source,
    _44_util_audio_to_interleaved_32f: *const c_void,
    _45_util_audio_from_interleaved_32f: *const c_void,
    _46_util_send_send_audio_interleaved_32f: *const c_void,
    /// slot 47
    pub recv_free_video_v2: unsafe extern "C" fn(Handle, *const VideoFrame),
    _48_recv_free_audio_v2: *const c_void,
    _49_recv_capture_v2: *const c_void,
    /// slot 50
    pub send_send_video_v2: unsafe extern "C" fn(Handle, *const VideoFrame),
    /// slot 51
    pub send_send_video_async_v2: unsafe extern "C" fn(Handle, *const VideoFrame),
    _52_send_send_audio_v2: *const c_void,
    _53_util_audio_to_interleaved_16s_v2: *const c_void,
    _54_util_audio_from_interleaved_16s_v2: *const c_void,
    _55_util_audio_to_interleaved_32f_v2: *const c_void,
    _56_util_audio_from_interleaved_32f_v2: *const c_void,
    _57_recv_free_string: *const c_void,
    _58_recv_ptz_is_supported: *const c_void,
    _59_recv_recording_is_supported: *const c_void,
    _60_recv_get_web_control: *const c_void,
    _61_recv_ptz_zoom: *const c_void,
    _62_recv_ptz_zoom_speed: *const c_void,
    _63_recv_ptz_pan_tilt: *const c_void,
    _64_recv_ptz_pan_tilt_speed: *const c_void,
    _65_recv_ptz_store_preset: *const c_void,
    _66_recv_ptz_recall_preset: *const c_void,
    _67_recv_ptz_auto_focus: *const c_void,
    _68_recv_ptz_focus: *const c_void,
    _69_recv_ptz_focus_speed: *const c_void,
    _70_recv_ptz_white_balance_auto: *const c_void,
    _71_recv_ptz_white_balance_indoor: *const c_void,
    _72_recv_ptz_white_balance_outdoor: *const c_void,
    _73_recv_ptz_white_balance_oneshot: *const c_void,
    _74_recv_ptz_white_balance_manual: *const c_void,
    _75_recv_ptz_exposure_auto: *const c_void,
    _76_recv_ptz_exposure_manual: *const c_void,
    _77_recv_recording_start: *const c_void,
    _78_recv_recording_stop: *const c_void,
    _79_recv_recording_set_audio_level: *const c_void,
    _80_recv_recording_is_recording: *const c_void,
    _81_recv_recording_get_filename: *const c_void,
    _82_recv_recording_get_error: *const c_void,
    _83_recv_recording_get_times: *const c_void,
    /// slot 84
    pub recv_create_v3: unsafe extern "C" fn(*const RecvCreate) -> Handle,
    /// slot 85
    pub recv_connect: unsafe extern "C" fn(Handle, *const Source),
    /// slot 86
    pub framesync_create: unsafe extern "C" fn(Handle) -> Handle,
    /// slot 87
    pub framesync_destroy: unsafe extern "C" fn(Handle),
    _88_framesync_capture_audio: *const c_void,
    _89_framesync_free_audio: *const c_void,
    /// slot 90
    pub framesync_capture_video: unsafe extern "C" fn(Handle, *mut VideoFrame, c_int),
    /// slot 91
    pub framesync_free_video: unsafe extern "C" fn(Handle, *mut VideoFrame),
    _92_util_send_send_audio_interleaved_32s: *const c_void,
    _93_util_audio_to_interleaved_32s_v2: *const c_void,
    _94_util_audio_from_interleaved_32s_v2: *const c_void,
    /// slot 95
    pub send_get_source_name: unsafe extern "C" fn(Handle) -> *const Source,
    /// slot 96
    pub send_send_audio_v3: unsafe extern "C" fn(Handle, *const AudioFrame),
    _97_util_V210_to_P216: *const c_void,
    _98_util_P216_to_V210: *const c_void,
    _99_routing_get_no_connections: *const c_void,
    _100_routing_get_source_name: *const c_void,
    /// slot 101
    pub recv_capture_v3: unsafe extern "C" fn(
        Handle,
        *mut VideoFrame,
        *mut AudioFrame,
        *mut MetadataFrame,
        u32,
    ) -> c_int,
    /// slot 102
    pub recv_free_audio_v3: unsafe extern "C" fn(Handle, *const AudioFrame),
    /// slot 103
    pub framesync_capture_audio_v2:
        unsafe extern "C" fn(Handle, *mut AudioFrame, c_int, c_int, c_int),
    /// slot 104
    pub framesync_free_audio_v2: unsafe extern "C" fn(Handle, *mut AudioFrame),
    /// slot 105
    pub framesync_audio_queue_depth: unsafe extern "C" fn(Handle) -> c_int,
    _106_recv_ptz_exposure_manual_v2: *const c_void,
}
pub const SLOTS: usize = 107;

/// The loaded and initialised SDK.
pub struct Ndi {
    lib: &'static Lib,
    _so: Library,
    pub path: PathBuf,
    pub version: String,
    finder: Mutex<Option<Finder>>,
}

// SAFETY: the function table is static and the SDK is thread safe for the
// calls made from several threads (each instance is used by one thread;
// the finder is behind a mutex).
unsafe impl Send for Ndi {}
unsafe impl Sync for Ndi {}

pub type NdiApi = Arc<Ndi>;

static LOADED: OnceLock<Result<NdiApi, String>> = OnceLock::new();

fn c(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

fn opt_c(s: Option<&str>) -> Option<CString> {
    s.filter(|s| !s.is_empty()).map(c)
}

fn ptr(s: &Option<CString>) -> *const c_char {
    s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr())
}

fn string(p: *const c_char) -> Option<String> {
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Where the SDK is looked for: `$NDI_RUNTIME_DIR_V6/libndi.so.6`, then the
/// linker path.
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = std::env::var_os(RUNTIME_DIR_VAR).filter(|d| !d.is_empty()) {
        v.push(PathBuf::from(d).join(LIBRARY));
    }
    v.push(PathBuf::from(LIBRARY));
    v
}

/// Load and initialise the SDK once. `NDI_CONFIG_DIR` (discovery server,
/// extra IPs) must be set before.
pub fn load() -> Result<NdiApi, String> {
    LOADED
        .get_or_init(|| unsafe {
            let mut errors = Vec::new();
            let mut found = None;
            for p in candidates() {
                match Library::new(&p) {
                    Ok(l) => {
                        found = Some((l, p));
                        break;
                    }
                    Err(e) => errors.push(format!("{}: {e}", p.display())),
                }
            }
            let (so, path) = found
                .ok_or_else(|| format!("cannot load the NDI runtime: {}", errors.join("; ")))?;
            let load: libloading::Symbol<unsafe extern "C" fn() -> *const Lib> = so
                .get(b"NDIlib_v5_load\0")
                .map_err(|e| format!("{}: NDIlib_v5_load: {e}", path.display()))?;
            let table = load();
            if table.is_null() {
                return Err(format!("{}: NDIlib_v5_load returned NULL", path.display()));
            }
            let lib: &'static Lib = &*table;
            if !(lib.is_supported_CPU)() {
                return Err("NDI: this CPU is not supported (SSE4.2 needed)".into());
            }
            if !(lib.initialize)() {
                return Err("NDIlib_initialize failed".into());
            }
            let version = string((lib.version)()).unwrap_or_default();
            Ok(Arc::new(Ndi {
                lib,
                _so: so,
                path,
                version,
                finder: Mutex::new(None),
            }))
        })
        .clone()
}

impl Ndi {
    pub fn lib(&self) -> &Lib {
        self.lib
    }

    /// Start the finder used for the source list (`/api/sources`).
    pub fn start_finder(&self, groups: Option<&str>, extra_ips: Option<&str>) {
        let g = opt_c(groups);
        let x = opt_c(extra_ips);
        let fc = FindCreate {
            show_local_sources: true,
            p_groups: ptr(&g),
            p_extra_ips: ptr(&x),
        };
        let h = unsafe { (self.lib.find_create_v2)(&fc) };
        if h.is_null() {
            tracing::warn!("NDIlib_find_create_v2 failed: no source list");
            return;
        }
        *self.finder.lock().unwrap_or_else(|e| e.into_inner()) = Some(Finder { h });
    }

    /// The NDI sources found now (`HOST (Name)`), with their URL.
    pub fn sources(&self) -> Vec<(String, String)> {
        let g = self.finder.lock().unwrap_or_else(|e| e.into_inner());
        let Some(f) = g.as_ref() else {
            return Vec::new();
        };
        let mut n = 0u32;
        // Valid until the next call: the mutex keeps callers apart.
        let p = unsafe { (self.lib.find_get_current_sources)(f.h, &mut n) };
        if p.is_null() {
            return Vec::new();
        }
        (0..n as usize)
            .filter_map(|i| {
                let s = unsafe { *p.add(i) };
                Some((
                    string(s.p_ndi_name)?,
                    string(s.p_url_address).unwrap_or_default(),
                ))
            })
            .collect()
    }
}

struct Finder {
    h: Handle,
}

/// Where an input connects: an NDI name (`HOST (Name)`, found by the SDK
/// through mDNS, a discovery server or the extra IPs), an address
/// (`ndi://ip:port` or `ip:port`) or both (`HOST (Name)@ip:port`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub name: Option<String>,
    pub url: Option<String>,
}

impl Address {
    pub fn parse(s: &str) -> Address {
        let s = s.trim();
        let s = s.strip_prefix("ndi://").unwrap_or(s);
        if let Some((n, u)) = s.rsplit_once('@') {
            if !u.is_empty() && !u.contains(' ') {
                return Address {
                    name: Some(n.trim().to_string()).filter(|n| !n.is_empty()),
                    url: Some(u.to_string()),
                };
            }
        }
        // `ip:port` / `host:port` without a name in brackets.
        if !s.contains('(') && s.contains(':') && !s.contains(' ') {
            return Address {
                name: None,
                url: Some(s.to_string()),
            };
        }
        Address {
            name: Some(s.to_string()),
            url: None,
        }
    }
}

/// A receiver (`NDIlib_recv_*`), optionally with a frame-sync. Used from one
/// thread.
pub struct Receiver {
    api: NdiApi,
    h: Handle,
    fs: Handle,
}

unsafe impl Send for Receiver {}

/// A captured frame; freed on drop.
pub enum Captured<'a> {
    None,
    Video(VideoGuard<'a>),
    Audio(AudioGuard<'a>),
    /// Metadata, status or source change
    Other(i32),
    /// The connection was lost
    Error,
}

pub struct VideoGuard<'a> {
    r: &'a Receiver,
    pub f: VideoFrame,
}

impl Drop for VideoGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.r.api.lib.recv_free_video_v2)(self.r.h, &self.f) }
    }
}

pub struct AudioGuard<'a> {
    r: &'a Receiver,
    pub f: AudioFrame,
}

impl Drop for AudioGuard<'_> {
    fn drop(&mut self) {
        unsafe { (self.r.api.lib.recv_free_audio_v3)(self.r.h, &self.f) }
    }
}

/// Video from the frame-sync; freed on drop.
pub struct FsVideo<'a> {
    r: &'a Receiver,
    pub f: VideoFrame,
}

impl Drop for FsVideo<'_> {
    fn drop(&mut self) {
        unsafe { (self.r.api.lib.framesync_free_video)(self.r.fs, &mut self.f) }
    }
}

pub struct FsAudio<'a> {
    r: &'a Receiver,
    pub f: AudioFrame,
}

impl Drop for FsAudio<'_> {
    fn drop(&mut self) {
        unsafe { (self.r.api.lib.framesync_free_audio_v2)(self.r.fs, &mut self.f) }
    }
}

impl Receiver {
    /// Connect to `address`; the SDK finds the source and reconnects by
    /// itself. `framesync`: put NDI's frame-sync (time-base corrector and
    /// audio resampler) between the receiver and the caller.
    pub fn new(
        api: &NdiApi,
        address: &Address,
        recv_name: &str,
        color_format: c_int,
        bandwidth: c_int,
        framesync: bool,
    ) -> Result<Receiver, String> {
        let name = opt_c(address.name.as_deref());
        let url = opt_c(address.url.as_deref());
        let rn = c(recv_name);
        let rc = RecvCreate {
            source_to_connect_to: Source {
                p_ndi_name: ptr(&name),
                p_url_address: ptr(&url),
            },
            color_format,
            bandwidth,
            allow_video_fields: true,
            p_ndi_recv_name: rn.as_ptr(),
        };
        let h = unsafe { (api.lib.recv_create_v3)(&rc) };
        if h.is_null() {
            return Err(format!("NDIlib_recv_create_v3({address:?}) failed"));
        }
        let mut r = Receiver {
            api: api.clone(),
            h,
            fs: std::ptr::null_mut(),
        };
        if framesync {
            r.fs = unsafe { (api.lib.framesync_create)(h) };
            if r.fs.is_null() {
                return Err("NDIlib_framesync_create failed".into());
            }
        }
        Ok(r)
    }

    pub fn has_framesync(&self) -> bool {
        !self.fs.is_null()
    }

    /// The next video or audio frame (metadata is dropped), waiting up to
    /// `timeout_ms`.
    pub fn capture(&self, timeout_ms: u32) -> Captured<'_> {
        let mut v = VideoFrame::default();
        let mut a = AudioFrame::default();
        let mut m = MetadataFrame::default();
        let t =
            unsafe { (self.api.lib.recv_capture_v3)(self.h, &mut v, &mut a, &mut m, timeout_ms) };
        match t {
            frame_type::NONE => Captured::None,
            frame_type::VIDEO => Captured::Video(VideoGuard { r: self, f: v }),
            frame_type::AUDIO => Captured::Audio(AudioGuard { r: self, f: a }),
            frame_type::METADATA => {
                unsafe { (self.api.lib.recv_free_metadata)(self.h, &m) };
                Captured::Other(t)
            }
            frame_type::ERROR => Captured::Error,
            t => Captured::Other(t),
        }
    }

    /// Frame-sync: the video frame to show now (`p_data` null before the
    /// first frame). The same frame may come back several times.
    pub fn fs_video(&self) -> FsVideo<'_> {
        let mut v = VideoFrame::default();
        unsafe {
            (self.api.lib.framesync_capture_video)(self.fs, &mut v, frame_format::PROGRESSIVE)
        };
        FsVideo { r: self, f: v }
    }

    /// Frame-sync: exactly `samples` samples of `channels` at `rate`
    /// (resampled to track the sender's clock; silence when none).
    pub fn fs_audio(&self, rate: i32, channels: i32, samples: i32) -> FsAudio<'_> {
        let mut a = AudioFrame::default();
        unsafe {
            (self.api.lib.framesync_capture_audio_v2)(self.fs, &mut a, rate, channels, samples)
        };
        FsAudio { r: self, f: a }
    }

    /// The source's current audio format through the frame-sync (rate 0
    /// when it has none).
    pub fn fs_audio_format(&self) -> (i32, i32) {
        let f = self.fs_audio(0, 0, 0);
        (f.f.sample_rate, f.f.no_channels)
    }

    pub fn fs_audio_queue_depth(&self) -> i32 {
        unsafe { (self.api.lib.framesync_audio_queue_depth)(self.fs) }
    }

    pub fn connections(&self) -> i32 {
        unsafe { (self.api.lib.recv_get_no_connections)(self.h) }
    }

    /// Total and dropped frames.
    pub fn performance(&self) -> (Performance, Performance) {
        let (mut t, mut d) = (Performance::default(), Performance::default());
        unsafe { (self.api.lib.recv_get_performance)(self.h, &mut t, &mut d) };
        (t, d)
    }

    pub fn queue(&self) -> Queue {
        let mut q = Queue::default();
        unsafe { (self.api.lib.recv_get_queue)(self.h, &mut q) };
        q
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        unsafe {
            if !self.fs.is_null() {
                (self.api.lib.framesync_destroy)(self.fs);
            }
            (self.api.lib.recv_destroy)(self.h);
        }
    }
}

/// A sender (`NDIlib_send_*`). Sent to from one thread; connections may be
/// read from another.
pub struct Sender {
    api: NdiApi,
    h: Handle,
}

unsafe impl Send for Sender {}
unsafe impl Sync for Sender {}

impl Sender {
    /// Not clocked: the output thread paces by TAI.
    pub fn new(api: &NdiApi, name: &str, groups: Option<&str>) -> Result<Sender, String> {
        let n = c(name);
        let g = opt_c(groups);
        let sc = SendCreate {
            p_ndi_name: n.as_ptr(),
            p_groups: ptr(&g),
            clock_video: false,
            clock_audio: false,
        };
        let h = unsafe { (api.lib.send_create)(&sc) };
        if h.is_null() {
            return Err(format!("NDIlib_send_create({name}) failed"));
        }
        Ok(Sender {
            api: api.clone(),
            h,
        })
    }

    /// The full NDI name (`HOST (Name)`).
    pub fn source_name(&self) -> String {
        let p = unsafe { (self.api.lib.send_get_source_name)(self.h) };
        if p.is_null() {
            return String::new();
        }
        string(unsafe { (*p).p_ndi_name }).unwrap_or_default()
    }

    /// Compress and send (synchronous: the data may be reused afterwards).
    pub fn send_video(&self, f: &VideoFrame) {
        unsafe { (self.api.lib.send_send_video_v2)(self.h, f) }
    }

    pub fn send_audio(&self, f: &AudioFrame) {
        unsafe { (self.api.lib.send_send_audio_v3)(self.h, f) }
    }

    pub fn connections(&self) -> i32 {
        unsafe { (self.api.lib.send_get_no_connections)(self.h, 0) }
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        unsafe { (self.api.lib.send_destroy)(self.h) }
    }
}

/// `ndi-config.v1.json` for the discovery server, the extra IPs and the
/// default groups. Written to `dir` and `NDI_CONFIG_DIR` pointed at it
/// before the SDK starts (the SDK reads it once, at initialisation).
pub fn config_json(
    discovery: Option<&str>,
    extra_ips: Option<&str>,
    groups: Option<&str>,
) -> serde_json::Value {
    let mut networks = serde_json::Map::new();
    if let Some(ips) = extra_ips.filter(|s| !s.is_empty()) {
        networks.insert("ips".into(), ips.into());
    }
    if let Some(d) = discovery.filter(|s| !s.is_empty()) {
        networks.insert("discovery".into(), d.into());
    }
    let mut ndi = serde_json::Map::new();
    if !networks.is_empty() {
        ndi.insert("networks".into(), networks.into());
    }
    if let Some(g) = groups.filter(|s| !s.is_empty()) {
        ndi.insert("groups".into(), serde_json::json!({"send": g, "recv": g}));
    }
    serde_json::json!({ "ndi": ndi })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn layout_matches_the_sdk_headers() {
        // x86-64 / aarch64 LP64.
        assert_eq!(size_of::<Source>(), 16);
        assert_eq!(offset_of!(VideoFrame, frame_format_type), 24);
        assert_eq!(offset_of!(VideoFrame, timecode), 32);
        assert_eq!(offset_of!(VideoFrame, p_data), 40);
        assert_eq!(offset_of!(VideoFrame, line_stride_in_bytes), 48);
        assert_eq!(offset_of!(VideoFrame, p_metadata), 56);
        assert_eq!(offset_of!(VideoFrame, timestamp), 64);
        assert_eq!(size_of::<VideoFrame>(), 72);
        assert_eq!(offset_of!(AudioFrame, timecode), 16);
        assert_eq!(offset_of!(AudioFrame, fourcc), 24);
        assert_eq!(offset_of!(AudioFrame, p_data), 32);
        assert_eq!(offset_of!(AudioFrame, channel_stride_in_bytes), 40);
        assert_eq!(offset_of!(AudioFrame, timestamp), 56);
        assert_eq!(size_of::<AudioFrame>(), 64);
        assert_eq!(size_of::<MetadataFrame>(), 24);
        assert_eq!(offset_of!(RecvCreate, color_format), 16);
        assert_eq!(offset_of!(RecvCreate, allow_video_fields), 24);
        assert_eq!(offset_of!(RecvCreate, p_ndi_recv_name), 32);
        assert_eq!(offset_of!(SendCreate, clock_video), 16);
        assert_eq!(offset_of!(SendCreate, clock_audio), 17);
        assert_eq!(size_of::<SendCreate>(), 24);
        assert_eq!(offset_of!(FindCreate, p_extra_ips), 16);
        // Function table: one pointer per slot of NDIlib_v5.
        assert_eq!(size_of::<Lib>(), SLOTS * 8);
        assert_eq!(offset_of!(Lib, recv_create_v3), 84 * 8);
        assert_eq!(offset_of!(Lib, framesync_create), 86 * 8);
        assert_eq!(offset_of!(Lib, send_get_source_name), 95 * 8);
        assert_eq!(offset_of!(Lib, recv_capture_v3), 101 * 8);
        assert_eq!(offset_of!(Lib, framesync_audio_queue_depth), 105 * 8);
        assert_eq!(fourcc::name(fourcc::UYVY), "UYVY");
        assert_eq!(fourcc::name(fourcc::P216), "P216");
        assert_eq!(fourcc::name(fourcc::FLTP), "FLTp");
    }

    #[test]
    fn addresses() {
        assert_eq!(
            Address::parse("MXL-TEST (NDI Out 1)"),
            Address {
                name: Some("MXL-TEST (NDI Out 1)".into()),
                url: None
            }
        );
        assert_eq!(
            Address::parse("ndi://10.0.0.5:5961"),
            Address {
                name: None,
                url: Some("10.0.0.5:5961".into())
            }
        );
        assert_eq!(
            Address::parse("CAM (Cam 1)@10.0.0.5:5961"),
            Address {
                name: Some("CAM (Cam 1)".into()),
                url: Some("10.0.0.5:5961".into())
            }
        );
    }

    #[test]
    fn config_file() {
        let j = config_json(Some("10.0.0.2:5959"), Some("10.0.0.7,10.0.0.8"), None);
        assert_eq!(j["ndi"]["networks"]["discovery"], "10.0.0.2:5959");
        assert_eq!(j["ndi"]["networks"]["ips"], "10.0.0.7,10.0.0.8");
        assert!(config_json(None, None, None)["ndi"]
            .as_object()
            .unwrap()
            .is_empty());
    }

    /// Against the real runtime (`NDI_RUNTIME_DIR_V6`):
    /// `cargo test --lib smoke -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn smoke_send_receive() {
        let api = load().expect("NDI runtime");
        println!("NDI {} from {}", api.version, api.path.display());
        let tx = Sender::new(&api, "lm-mxl-ndi smoke", None).unwrap();
        let name = tx.source_name();
        println!("sender {name}");
        let rx = Receiver::new(
            &api,
            &Address::parse(&name),
            "smoke rx",
            color_format::BEST,
            bandwidth::HIGHEST,
            false,
        )
        .unwrap();
        let (w, h) = (1920usize, 1080usize);
        for (fcc, label) in [(fourcc::UYVY, "8-bit"), (fourcc::P216, "16-bit")] {
            let bytes = if fcc == fourcc::P216 {
                4 * w * h
            } else {
                2 * w * h
            };
            let mut data = vec![0u8; bytes];
            if fcc == fourcc::P216 {
                let (y, c) = data.split_at_mut(2 * w * h);
                for (i, v) in y.chunks_exact_mut(2).enumerate() {
                    v.copy_from_slice(
                        &(((64 + (i % 1920) * 876 / 1919) as u16) << 6).to_le_bytes(),
                    );
                }
                for v in c.chunks_exact_mut(2) {
                    v.copy_from_slice(&(512u16 << 6).to_le_bytes());
                }
            } else {
                for (i, v) in data.chunks_exact_mut(2).enumerate() {
                    v[0] = 128;
                    v[1] = (16 + (i % 1920) * 219 / 1919) as u8;
                }
            }
            let mut pcm = vec![0.25f32; 960 * 2];
            let mut got = None;
            let mut audio = None;
            let t0 = std::time::Instant::now();
            let mut n = 0i64;
            while t0.elapsed().as_secs() < 8 && (got.is_none() || audio.is_none()) {
                n += 1;
                if rx.connections() > 0 || n % 10 == 0 {
                    tx.send_video(&VideoFrame {
                        xres: w as i32,
                        yres: h as i32,
                        fourcc: fcc,
                        frame_rate_n: 50,
                        frame_rate_d: 1,
                        picture_aspect_ratio: 16.0 / 9.0,
                        frame_format_type: frame_format::PROGRESSIVE,
                        timecode: 1_000_000 + n,
                        p_data: data.as_mut_ptr(),
                        line_stride_in_bytes: (2 * w) as i32,
                        p_metadata: std::ptr::null(),
                        timestamp: 0,
                    });
                    tx.send_audio(&AudioFrame {
                        sample_rate: 48_000,
                        no_channels: 2,
                        no_samples: 960,
                        timecode: 1_000_000 + n,
                        fourcc: fourcc::FLTP,
                        p_data: pcm.as_mut_ptr() as *mut u8,
                        channel_stride_in_bytes: 960 * 4,
                        p_metadata: std::ptr::null(),
                        timestamp: 0,
                    });
                }
                match rx.capture(20) {
                    Captured::Video(g) if got.is_none() => {
                        let f = g.f;
                        let first = unsafe { std::slice::from_raw_parts(f.p_data, 16) }.to_vec();
                        got = Some((
                            fourcc::name(f.fourcc),
                            f.xres,
                            f.yres,
                            f.line_stride_in_bytes,
                            f.timecode,
                            f.timestamp,
                            first,
                        ));
                    }
                    Captured::Audio(g) if audio.is_none() => {
                        let f = g.f;
                        let v = unsafe { *(f.p_data as *const f32) };
                        audio = Some((
                            fourcc::name(f.fourcc),
                            f.no_channels,
                            f.no_samples,
                            f.channel_stride_in_bytes,
                            v,
                            f.timecode,
                        ));
                    }
                    _ => {}
                }
            }
            println!("{label}: video {got:?}");
            println!("{label}: audio {audio:?}");
            assert!(got.is_some() && audio.is_some());
        }
    }
}
