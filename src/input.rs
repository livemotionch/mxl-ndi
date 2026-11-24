//! NDI → MXL: one thread per input. It receives from an NDI source (the SDK
//! decodes SpeedHQ to UYVY or P216 in its own `ndir:video` threads), converts to v210
//! and writes a video grain and float32 samples into the input's MXL flows,
//! which are always there (black and silence without a source).
//!
//! Two ways to put frames on the MXL grain clock (`NDI_IN_SYNC`):
//!
//! `tai` (default), the OMT bridge's placement. NDI frames carry two times:
//! the `timecode` the sender sets (this bridge's outputs put the grain's TAI
//! time there) and the `timestamp` the sending SDK sets to the UTC time the
//! frame was submitted. An input uses the timecode when it is a plausible
//! TAI time, else the timestamp (+ the TAI−UTC offset), else the arrival. At
//! the first frame it picks a delay `D` = arrival − sender time, plus
//! `NDI_IN_DELAY_MS`. A video frame of sender time `ts` is written to the
//! grain whose period holds `ts + D`, and audio samples to the sample index
//! of `ts + D`. So video and audio keep the sync they had at the sender, and
//! arrival jitter doesn't move frames between grains. A sender with another
//! rate or a free-running clock comes out frame-synchronised: two frames for
//! one grain → the later one is dropped; a grain without a frame → the last
//! one is repeated. `D` is picked again (a resync) when frames keep coming a
//! grain later than their mapping, or when a delay that puts frames a grain
//! earlier would have fitted every frame of the last two seconds (latency
//! trim), or the source clock jumps.
//!
//! `framesync`: NDI's own frame-sync (`NDIlib_framesync_*`, a time-base
//! corrector with dynamic audio resampling). Once per grain, just after its
//! start, the input pulls the frame to show and exactly one grain of audio
//! and writes them to that grain. Drift of the sender's clock is absorbed by
//! the SDK (audio resampled, frames repeated or dropped), at the price of the
//! frame-sync's buffering.
//!
//! Loss: grains nobody wrote are filled one grain late (half a period of
//! slack) with the last frame for `NDI_IN_HOLD_MS` (`NDI_IN_LOSS=hold`),
//! then black; audio is silence when none came for 100 ms.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crate::config::{Config, LossMode, SyncMode};
use crate::convert::{self, Planar};
use crate::mxlio::{self, flowdef, Api, Clock, Instance, RawWriter};
use crate::ndi::{self, fourcc, Captured, NdiApi};
use crate::state::{lock, Shared};

pub fn video_flow_id(seed: &str, i: usize) -> uuid::Uuid {
    mxl_nmos::ids::derive_id(seed, &format!("in-{}/video/mxl-flow", i + 1))
}

pub fn audio_flow_id(seed: &str, i: usize) -> uuid::Uuid {
    mxl_nmos::ids::derive_id(seed, &format!("in-{}/audio/mxl-flow", i + 1))
}

pub fn labels(config: &Config, i: usize) -> (String, String) {
    (
        format!("{} In {} Video", config.label, i + 1),
        format!("{} In {} Audio", config.label, i + 1),
    )
}

const AUDIO_RATE: (u32, u32) = (48_000, 1);
/// Audio more than this off its contiguous position is placed again.
const AUDIO_SLIP: i64 = 480;
/// A timecode within this of TAI now is taken as a TAI time (ns).
const TAI_PLAUSIBLE_NS: i64 = 5_000_000_000;
/// Frame-sync mode: pull a grain's frame this long after the grain starts.
const FS_PULL_OFFSET_NS: u64 = 1_000_000;

/// Which sender time an input places frames by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeBase {
    /// NDI timecode carrying TAI (this bridge's outputs)
    Timecode,
    /// The sending SDK's submit time (UTC → TAI)
    Timestamp,
    /// No usable sender time: the arrival
    Arrival,
}

impl TimeBase {
    fn name(self) -> &'static str {
        match self {
            TimeBase::Timecode => "timecode",
            TimeBase::Timestamp => "timestamp",
            TimeBase::Arrival => "arrival",
        }
    }
}

struct Timing {
    /// TAI ns − sender ns
    delay: i64,
    /// Consecutive frames more than a grain later than their mapping
    late_run: u32,
    /// Largest transit in the current trim window, and the window's start
    win_max: i64,
    win_start: Instant,
}

/// Per-second counters for rates.
#[derive(Default)]
struct Second {
    frames_in: u64,
    grains: u64,
    convert_ns: u64,
    converts: u64,
    capture_ns: u64,
    captures: u64,
    transit_sum: f64,
    transit_n: u64,
    transit_max: f64,
    latency_sum: f64,
    latency_n: u64,
}

pub struct Input {
    i: usize,
    config: Arc<Config>,
    shared: Arc<Shared>,
    ndi: NdiApi,
    clock: Clock,
    rate: mxl_sys::Rational,
    arate: mxl_sys::Rational,
    period: u64,
    video: RawWriter,
    audio: RawWriter,
    channels: usize,
    w: usize,
    h: usize,
    /// The last frame in v210, and black
    held: Vec<u8>,
    black: Vec<u8>,
    have_held: bool,
    planar_in: Planar,
    planar_out: Planar,
    receiver: Option<ndi::Receiver>,
    source: String,
    time_base: Option<TimeBase>,
    /// Frames seen without a plausible TAI timecode, time base undecided
    undecided: u32,
    timing: Option<Timing>,
    tai_offset_ns: i64,
    last_written: u64,
    last_frame: Option<Instant>,
    next_sample: Option<u64>,
    /// Grain time − sender time of the video frames (smoothed): where the
    /// audio goes, so it lands exactly as late as the picture
    av_shift: Option<i64>,
    audio_realign: bool,
    last_audio: Option<Instant>,
    /// Frame-sync: the next grain to pull, and the last frame's identity
    fs_next: u64,
    fs_last: Option<(i64, i64)>,
    fs_audio: Vec<f32>,
    sec: Second,
    sec_start: Instant,
}

impl Input {
    pub fn new(
        api: &Api,
        ndi: &NdiApi,
        domain: &Path,
        config: Arc<Config>,
        shared: Arc<Shared>,
        i: usize,
    ) -> Result<Input, String> {
        let inst = Instance::new(api, domain)?;
        let f = config.format;
        let (vl, al) = labels(&config, i);
        let vdef = flowdef::video(
            video_flow_id(&config.seed, i),
            &vl,
            &format!("NDI input {} (lm-mxl-ndi)", i + 1),
            &config.nmos_label,
            &f,
            false,
        );
        let (video, _) = inst.writer(&vdef).map_err(|e| format!("{vl}: {e}"))?;
        let adef = flowdef::audio(
            audio_flow_id(&config.seed, i),
            &al,
            &format!("NDI input {} audio (lm-mxl-ndi)", i + 1),
            &config.nmos_label,
            config.audio_channels,
        );
        let (audio, _) = inst.writer(&adef).map_err(|e| format!("{al}: {e}"))?;
        let (w, h) = (f.width as usize, f.height as usize);
        let mut black = vec![0u8; convert::v210_frame_bytes(w, h)];
        convert::v210_black(w, h, &mut black);
        {
            let mut st = lock(&shared.inputs[i].status);
            st.index = i + 1;
            st.video_flow = video.id().to_string();
            st.audio_flow = audio.id().to_string();
            st.state = "no_source".into();
        }
        Ok(Input {
            i,
            channels: config.audio_channels,
            rate: mxlio::rational(f.rate_n, f.rate_d),
            arate: mxlio::rational(AUDIO_RATE.0, AUDIO_RATE.1),
            period: f.frame_ns(),
            config,
            shared,
            ndi: ndi.clone(),
            clock: Clock::new(api.clone()),
            video,
            audio,
            w,
            h,
            held: black.clone(),
            black,
            have_held: false,
            planar_in: Planar::default(),
            planar_out: Planar::default(),
            receiver: None,
            source: String::new(),
            time_base: None,
            undecided: 0,
            timing: None,
            tai_offset_ns: mxl_nmos::clock::tai_utc_offset_secs() * 1_000_000_000,
            last_written: 0,
            last_frame: None,
            next_sample: None,
            av_shift: None,
            audio_realign: false,
            last_audio: None,
            fs_next: 0,
            fs_last: None,
            fs_audio: Vec::new(),
            sec: Second::default(),
            sec_start: Instant::now(),
        })
    }

    pub fn run(mut self) {
        while !self.shared.stopping() {
            self.check_source();
            match self.config.in_sync {
                SyncMode::Tai => self.tai_step(),
                SyncMode::Framesync => self.fs_step(),
            }
            self.fill();
            if self.sec_start.elapsed().as_secs_f64() >= 1.0 {
                self.publish();
            }
        }
    }

    /// TAI mode: take the next frame (or time out at the next fill).
    fn tai_step(&mut self) {
        // Wait for a frame at most until the next grain may need filling.
        let now = self.clock.now();
        let next_fill = self
            .clock
            .index_to_time(&self.rate, self.clock.index_now(&self.rate) + 1)
            + self.period / 2;
        let timeout_ms = ((next_fill.saturating_sub(now)) / 1_000_000).clamp(1, 40) as u32;
        let Some(r) = self.receiver.take() else {
            std::thread::sleep(std::time::Duration::from_millis(timeout_ms as u64));
            return;
        };
        // A frame already queued: what the call costs is the decode.
        let t0 = Instant::now();
        {
            let mut got = r.capture(0);
            let queued = !matches!(got, Captured::None);
            if !queued {
                got = r.capture(timeout_ms);
            }
            match got {
                Captured::Video(g) => {
                    if queued {
                        self.sec.capture_ns += t0.elapsed().as_nanos() as u64;
                        self.sec.captures += 1;
                    }
                    let f = g.f;
                    self.on_video(&f);
                }
                Captured::Audio(g) => {
                    let f = g.f;
                    self.on_audio(&f);
                }
                Captured::Error => {
                    lock(&self.shared.inputs[self.i].status).error =
                        Some("NDI connection lost".into());
                }
                Captured::None | Captured::Other(_) => {}
            }
        }
        self.receiver = Some(r);
    }

    fn check_source(&mut self) {
        let want = lock(&self.shared.inputs[self.i].source).clone();
        if want == self.source {
            return;
        }
        tracing::info!("in-{}: source {:?}", self.i + 1, want);
        self.receiver = None;
        self.source = want.clone();
        self.timing = None;
        self.time_base = None;
        self.undecided = 0;
        self.next_sample = None;
        self.av_shift = None;
        self.fs_last = None;
        if want.is_empty() {
            // No source: black, not the last picture of the old one.
            self.have_held = false;
            self.last_frame = None;
        }
        {
            let mut st = lock(&self.shared.inputs[self.i].status);
            st.source = want.clone();
            st.error = None;
            st.sender = None;
            st.timing = None;
            st.codec = None;
            st.frame_rate = None;
            st.audio_rate = 0;
            st.audio_channels = 0;
            st.scaled = false;
            st.state = if want.is_empty() {
                "no_source"
            } else {
                "connecting"
            }
            .into();
        }
        if want.is_empty() {
            return;
        }
        let address = ndi::Address::parse(&want);
        match ndi::Receiver::new(
            &self.ndi,
            &address,
            &format!("{} In {}", self.config.label, self.i + 1),
            self.config.in_color,
            self.config.in_bandwidth,
            self.config.in_sync == SyncMode::Framesync,
        ) {
            Ok(r) => {
                self.receiver = Some(r);
                self.fs_next = self.clock.index_now(&self.rate) + 1;
            }
            Err(e) => lock(&self.shared.inputs[self.i].status).error = Some(e),
        }
    }

    fn set_error(&self, e: Option<String>) {
        let mut st = lock(&self.shared.inputs[self.i].status);
        if st.error != e {
            if let Some(e) = &e {
                tracing::warn!("in-{}: {e}", self.i + 1);
            }
            st.error = e;
        }
    }

    /// The sender time (TAI ns) of a frame, picking the time base at the
    /// start of a connection. `None`: skip the frame. A new connection to a
    /// sender that was idle first gets the sender's last frame, seconds old:
    /// such a frame neither decides the time base nor is written.
    fn sender_time(&mut self, timecode: i64, timestamp: i64, arrival: u64) -> Option<i64> {
        let tc_age = timecode.saturating_mul(100).saturating_sub(arrival as i64);
        let tc_set = timecode != ndi::TIMECODE_SYNTHESIZE && timecode > 0;
        let base = match self.time_base {
            Some(b) => b,
            None => {
                let b = if tc_set && tc_age.abs() < TAI_PLAUSIBLE_NS {
                    TimeBase::Timecode
                } else {
                    self.undecided += 1;
                    if self.undecided < 10 {
                        return None;
                    }
                    if timestamp != ndi::TIMESTAMP_UNDEFINED && timestamp > 0 {
                        TimeBase::Timestamp
                    } else {
                        TimeBase::Arrival
                    }
                };
                tracing::info!(
                    "in-{}: placing frames by the sender's {}",
                    self.i + 1,
                    b.name()
                );
                lock(&self.shared.inputs[self.i].status).timing = Some(b.name().into());
                self.time_base = Some(b);
                b
            }
        };
        match base {
            // A stale frame (more than a second old): not for any grain.
            TimeBase::Timecode if tc_age < -1_000_000_000 => None,
            TimeBase::Timecode => Some(timecode.saturating_mul(100)),
            TimeBase::Timestamp if timestamp != ndi::TIMESTAMP_UNDEFINED => {
                Some(timestamp.saturating_mul(100) + self.tai_offset_ns)
            }
            _ => Some(arrival as i64),
        }
    }

    /// Map a sender time (TAI ns) to TAI, picking or adjusting the delay.
    fn map(&mut self, ts_ns: i64, arrival: u64, video: bool) -> u64 {
        let transit = (arrival as i64).saturating_sub(ts_ns);
        let p = self.period as i64;
        let extra = self.config.in_delay_ns as i64;
        let mut resync = false;
        match &mut self.timing {
            None => {
                self.timing = Some(Timing {
                    delay: transit + extra,
                    late_run: 0,
                    win_max: transit,
                    win_start: Instant::now(),
                });
            }
            Some(t) if video => {
                let late = transit + extra - t.delay;
                // The source clock jumped (or restarted): place again now.
                if late.abs() > 4 * p {
                    t.delay = transit + extra;
                    resync = true;
                } else if late > p {
                    t.late_run += 1;
                    if t.late_run >= 3 {
                        t.delay = transit + extra;
                        t.late_run = 0;
                        resync = true;
                    }
                } else {
                    t.late_run = 0;
                }
                t.win_max = t.win_max.max(transit);
                if t.win_start.elapsed().as_secs() >= 2 {
                    // Trim when the slowest frame of the window would still
                    // land in time with a delay that puts this frame a
                    // grain earlier (a delay picked in a startup burst).
                    let shorter = t.win_max + extra;
                    if shorter < t.delay
                        && (ts_ns + shorter).div_euclid(p) < (ts_ns + t.delay).div_euclid(p)
                    {
                        t.delay = shorter;
                        resync = true;
                    }
                    t.win_max = transit;
                    t.win_start = Instant::now();
                }
            }
            Some(_) => {}
        }
        if resync {
            self.av_shift = None;
            lock(&self.shared.inputs[self.i].status).resyncs += 1;
        }
        let d = self.timing.as_ref().unwrap().delay;
        if video && self.time_base != Some(TimeBase::Arrival) && transit.abs() < 10_000_000_000 {
            let ms = transit as f64 / 1e6;
            self.sec.transit_sum += ms;
            self.sec.transit_n += 1;
            self.sec.transit_max = self.sec.transit_max.max(ms);
        }
        (ts_ns + d).max(0) as u64
    }

    /// Note a video frame's format in the status.
    fn note_format(&self, f: &ndi::VideoFrame) {
        let mut st = lock(&self.shared.inputs[self.i].status);
        st.frames_received += 1;
        st.codec = Some(fourcc::name(f.fourcc));
        st.width = f.xres.max(0) as u32;
        st.height = f.yres.max(0) as u32;
        st.frame_rate = Some(if f.frame_rate_d <= 1 {
            format!("{}", f.frame_rate_n)
        } else {
            format!(
                "{:.2}",
                f.frame_rate_n as f64 / f.frame_rate_d.max(1) as f64
            )
        });
        st.interlaced = f.frame_format_type != ndi::frame_format::PROGRESSIVE;
        if f.timecode != ndi::TIMECODE_SYNTHESIZE {
            st.video_ts_phase_ms =
                (f.timecode.saturating_mul(100)).rem_euclid(self.period as i64) as f64 / 1e6;
        }
    }

    /// Convert an NDI frame to v210 into `held`. False: not usable.
    fn convert(&mut self, f: &ndi::VideoFrame) -> bool {
        let (fw, fh) = (f.xres.max(0) as usize, f.yres.max(0) as usize);
        if f.p_data.is_null() || fw == 0 || fh == 0 {
            return false;
        }
        let t0 = Instant::now();
        let stride = f.line_stride_in_bytes.max(0) as usize;
        let p216 = f.fourcc == fourcc::P216 || f.fourcc == fourcc::PA16;
        let uyvy = f.fourcc == fourcc::UYVY || f.fourcc == fourcc::UYVA;
        // P216: a Y plane, then the CbCr plane, both `fh` rows of `stride`
        // (the alpha plane of PA16 / UYVA follows and is ignored).
        let need = if p216 { stride * fh * 2 } else { stride * fh };
        let data = unsafe { std::slice::from_raw_parts(f.p_data as *const u8, need) };
        // A single field (allow_video_fields is implied by best/fastest):
        // half height, scaled up to the full frame (bob).
        let mut scaled = false;
        let ok = if !(p216 || uyvy) {
            self.set_error(Some(format!(
                "NDI format {} is not supported (UYVY, P216)",
                fourcc::name(f.fourcc)
            )));
            false
        } else if stride < 2 * fw {
            self.set_error(Some(format!("short rows: stride {stride} for {fw}x{fh}")));
            false
        } else if (fw, fh) == (self.w, self.h) {
            if p216 {
                convert::p216_to_v210(data, stride, fw, fh, &mut self.held);
            } else {
                convert::uyvy_to_v210(data, stride, fw, fh, &mut self.held);
            }
            true
        } else if self.config.in_scale && (fw, fh) == (2 * self.w, 2 * self.h) {
            convert::half_to_v210(data, stride, self.w, self.h, p216, &mut self.held);
            scaled = true;
            true
        } else if self.config.in_scale {
            self.planar_in.load(data, stride, fw, fh, p216);
            self.planar_in
                .scale_into(&mut self.planar_out, self.w, self.h);
            self.planar_out.to_v210(&mut self.held);
            scaled = true;
            true
        } else {
            self.set_error(Some(format!(
                "{fw}x{fh} is not the house format {}x{} (NDI_IN_SCALE=false)",
                self.w, self.h
            )));
            false
        };
        self.sec.convert_ns += t0.elapsed().as_nanos() as u64;
        self.sec.converts += 1;
        if !ok {
            self.have_held = false;
            lock(&self.shared.inputs[self.i].status).state = "unsupported".into();
            return false;
        }
        if lock(&self.shared.inputs[self.i].status).error.is_some() {
            self.set_error(None);
        }
        lock(&self.shared.inputs[self.i].status).scaled = scaled;
        true
    }

    fn on_video(&mut self, f: &ndi::VideoFrame) {
        let arrival = self.clock.now();
        self.sec.frames_in += 1;
        self.note_format(f);
        if f.p_data.is_null() || f.xres <= 0 || f.yres <= 0 {
            return;
        }
        let Some(ts) = self.sender_time(f.timecode, f.timestamp, arrival) else {
            lock(&self.shared.inputs[self.i].status).stale += 1;
            return;
        };
        let target = self.map(ts, arrival, true);
        let mut j = self.clock.time_to_index(&self.rate, target);
        let now_idx = self.clock.index_now(&self.rate);
        if j + 3 < now_idx || j > now_idx + 3 {
            // Mapping far off (a delay picked from a stalled first frame):
            // place this frame now and pick the delay again.
            if let Some(t) = &mut self.timing {
                t.delay += (now_idx as i64 - j as i64) * self.period as i64;
            }
            self.av_shift = None;
            lock(&self.shared.inputs[self.i].status).resyncs += 1;
            j = now_idx;
        }
        if j <= self.last_written {
            // A second frame for a grain already written, or too late.
            lock(&self.shared.inputs[self.i].status).dropped += 1;
            return;
        }
        if !self.convert(f) {
            return;
        }
        // Grains between the last written and this one that the filler
        // didn't reach yet: the new frame.
        if self.last_written > 0 && j > self.last_written + 1 && j - self.last_written <= 3 {
            for k in self.last_written + 1..j {
                self.write_video(k, false);
                lock(&self.shared.inputs[self.i].status).repeated += 1;
            }
        }
        self.have_held = true;
        self.last_frame = Some(Instant::now());
        self.write_video(j, false);
        let t_j = self.clock.index_to_time(&self.rate, j) as i64;
        let shift = t_j - ts;
        // Frame-synchronised sources shift by up to a grain from frame to
        // frame: the audio follows the average.
        self.av_shift = Some(match self.av_shift {
            None => {
                // Audio written so far went by the raw delay: place again.
                self.audio_realign = true;
                shift
            }
            Some(a) => a + (shift - a) / 16,
        });
        let lat = shift as f64 / 1e6;
        if self.time_base != Some(TimeBase::Arrival) && lat.abs() < 10_000.0 {
            self.sec.latency_sum += lat;
            self.sec.latency_n += 1;
        }
        lock(&self.shared.inputs[self.i].status).state = "ok".into();
    }

    fn write_video(&mut self, index: u64, black: bool) {
        let src = if black { &self.black } else { &self.held };
        match self.video.write_grain(index, |p| {
            let n = p.len().min(src.len());
            p[..n].copy_from_slice(&src[..n]);
        }) {
            Ok(()) => {
                self.last_written = index;
                self.sec.grains += 1;
                let mut st = lock(&self.shared.inputs[self.i].status);
                st.grains_written += 1;
                if black {
                    st.black += 1;
                }
            }
            Err(e) => self.set_error(Some(format!("MXL video write {index}: {e}"))),
        }
    }

    /// Note an audio frame's format; false: not 48 kHz FLTP.
    fn note_audio(&self, f: &ndi::AudioFrame) -> bool {
        let mut st = lock(&self.shared.inputs[self.i].status);
        st.audio_frames += 1;
        st.audio_rate = f.sample_rate.max(0) as u32;
        st.audio_channels = f.no_channels.max(0) as u32;
        st.audio_samples = f.no_samples.max(0) as u32;
        if f.timecode != ndi::TIMECODE_SYNTHESIZE {
            st.audio_ts_phase_ms =
                (f.timecode.saturating_mul(100)).rem_euclid(self.period as i64) as f64 / 1e6;
        }
        f.fourcc == fourcc::FLTP
            && f.sample_rate == 48_000
            && !f.p_data.is_null()
            && f.no_samples > 0
    }

    fn on_audio(&mut self, f: &ndi::AudioFrame) {
        let arrival = self.clock.now();
        if !self.note_audio(f) {
            return;
        }
        let n = f.no_samples as usize;
        let ch = f.no_channels.max(0) as usize;
        let stride = (f.channel_stride_in_bytes.max(0) as usize) / 4;
        if ch > 0 && stride < n {
            return;
        }
        let Some(ts) = self.sender_time(f.timecode, f.timestamp, arrival) else {
            return;
        };
        let mut target = self.map(ts, arrival, false);
        if let Some(shift) = self.av_shift {
            target = (ts + shift).max(0) as u64;
        }
        let s0 = self.clock.time_to_index(&self.arate, target);
        // SAFETY: FLTP, `ch` channels of `stride` floats each.
        let data =
            unsafe { std::slice::from_raw_parts(f.p_data as *const f32, stride * ch.max(1)) };
        let mut skip = 0usize;
        let slip = if std::mem::take(&mut self.audio_realign) {
            0
        } else {
            AUDIO_SLIP
        };
        let start = match self.next_sample {
            Some(next) if (s0 as i64 - next as i64).abs() <= slip => next,
            Some(next) if s0 > next && s0 - next < 48_000 => {
                // A gap: silence up to the new position.
                lock(&self.shared.inputs[self.i].status).audio_resyncs += 1;
                self.write_silence(next, (s0 - next) as usize);
                s0
            }
            Some(next) if s0 < next => {
                // Before what is written (MXL writes only forward): the
                // part after it, if any.
                lock(&self.shared.inputs[self.i].status).audio_resyncs += 1;
                if next - s0 >= n as u64 {
                    return;
                }
                skip = (next - s0) as usize;
                next
            }
            Some(_) => {
                // Far ahead (more than a second): start there.
                lock(&self.shared.inputs[self.i].status).audio_resyncs += 1;
                s0
            }
            None => s0,
        };
        self.write_planar(start, data, stride, ch, skip, n - skip);
    }

    /// Write `count` samples per channel from planar `data` (channel `c` at
    /// `c * stride`, from sample `skip`) at sample index `start`.
    fn write_planar(
        &mut self,
        start: u64,
        data: &[f32],
        stride: usize,
        ch: usize,
        skip: usize,
        count: usize,
    ) {
        if count == 0 {
            return;
        }
        let channels = self.channels;
        let r = self
            .audio
            .write_samples(start + count as u64, count, |slices| {
                for (c, (a, b)) in slices.iter_mut().enumerate().take(channels) {
                    let src: &[f32] = if c < ch {
                        &data[c * stride + skip..c * stride + skip + count]
                    } else {
                        &[]
                    };
                    let la = a.len();
                    for (k, v) in a.iter_mut().enumerate() {
                        *v = src.get(k).copied().unwrap_or(0.0);
                    }
                    for (k, v) in b.iter_mut().enumerate() {
                        *v = src.get(la + k).copied().unwrap_or(0.0);
                    }
                }
            });
        match r {
            Ok(()) => {
                self.next_sample = Some(start + count as u64);
                self.last_audio = Some(Instant::now());
            }
            Err(e) => self.set_error(Some(format!("MXL audio write at {start}+{count}: {e}"))),
        }
    }

    /// Frame-sync mode: per grain, pull a frame and one grain of audio.
    fn fs_step(&mut self) {
        let k = self
            .fs_next
            .max(self.clock.index_now(&self.rate).saturating_sub(2));
        let due = self.clock.index_to_time(&self.rate, k) + FS_PULL_OFFSET_NS;
        // Sleep in steps so that a source change or stop is seen quickly.
        let now = self.clock.now();
        if now < due {
            self.clock.sleep_until(due.min(now + 20_000_000));
            if self.clock.now() < due {
                return;
            }
        }
        self.fs_next = k + 1;
        let Some(r) = self.receiver.take() else {
            return;
        };
        if k > self.last_written {
            self.fs_video(&r, k);
        }
        self.fs_audio_grain(&r, k);
        self.receiver = Some(r);
    }

    fn fs_video(&mut self, r: &ndi::Receiver, k: u64) {
        let t0 = Instant::now();
        let v = r.fs_video();
        let f = v.f;
        if f.p_data.is_null() {
            // No frame yet: the filler writes black.
            return;
        }
        let id = (f.timecode, f.timestamp);
        let new = self.fs_last != Some(id);
        let t_k = self.clock.index_to_time(&self.rate, k);
        if new {
            self.sec.capture_ns += t0.elapsed().as_nanos() as u64;
            self.sec.captures += 1;
            self.sec.frames_in += 1;
            self.note_format(&f);
            // Frames the frame-sync skipped (a faster source, or it caught up).
            if let (Some((last_tc, _)), true) = (self.fs_last, f.frame_rate_n > 0) {
                let src_period = 1e7 * f.frame_rate_d as f64 / f.frame_rate_n as f64;
                let gap = (f.timecode - last_tc) as f64 / src_period;
                if gap > 1.5 && gap < 1000.0 {
                    lock(&self.shared.inputs[self.i].status).dropped +=
                        (gap.round() as u64).saturating_sub(1);
                }
            }
            self.fs_last = Some(id);
            if self.time_base.is_none() {
                let tc_ok = f.timecode != ndi::TIMECODE_SYNTHESIZE
                    && (f.timecode.saturating_mul(100) - t_k as i64).abs() < TAI_PLAUSIBLE_NS;
                self.time_base = Some(if tc_ok {
                    TimeBase::Timecode
                } else {
                    TimeBase::Timestamp
                });
                lock(&self.shared.inputs[self.i].status).timing = Some("framesync".into());
            }
            if !self.convert(&f) {
                return;
            }
            self.have_held = true;
            self.last_frame = Some(Instant::now());
        } else {
            // The frame-sync repeated its last frame.
            lock(&self.shared.inputs[self.i].status).repeated += 1;
            if !self.have_held {
                return;
            }
            if self
                .last_frame
                .is_some_and(|t| t.elapsed() >= self.config.in_hold)
            {
                // Lost: let the filler go to black.
                return;
            }
        }
        drop(v);
        // Latency against the sender's grain time (this bridge's outputs).
        let ts = match self.time_base {
            Some(TimeBase::Timecode) => Some(f.timecode.saturating_mul(100)),
            _ if f.timestamp != ndi::TIMESTAMP_UNDEFINED && f.timestamp > 0 => {
                Some(f.timestamp.saturating_mul(100) + self.tai_offset_ns)
            }
            _ => None,
        };
        if let Some(ts) = ts {
            let lat = (t_k as i64 - ts) as f64 / 1e6;
            if lat.abs() < 10_000.0 && new {
                self.sec.latency_sum += lat;
                self.sec.latency_n += 1;
            }
        }
        self.write_video(k, false);
        let mut st = lock(&self.shared.inputs[self.i].status);
        st.state = if new
            || self
                .last_frame
                .is_some_and(|t| t.elapsed().as_millis() < 3 * self.period as u128 / 1_000_000)
        {
            "ok"
        } else {
            "holding"
        }
        .into();
    }

    fn fs_audio_grain(&mut self, r: &ndi::Receiver, k: u64) {
        let t_k = self.clock.index_to_time(&self.rate, k);
        let s0 = self.clock.time_to_index(&self.arate, t_k);
        let s1 = self.clock.time_to_index(&self.arate, t_k + self.period);
        let count = (s1 - s0) as usize;
        if count == 0 || count > 48_000 {
            return;
        }
        // Up to what is written already (MXL writes forward only).
        let start = match self.next_sample {
            Some(n) if n >= s1 => return,
            Some(n) if n > s0 => n,
            Some(n) if n < s0 && s0 - n < 48_000 => {
                self.write_silence(n, (s0 - n) as usize);
                s0
            }
            _ => s0,
        };
        let (rate, _) = r.fs_audio_format();
        if rate == 0 {
            // No audio from the source: the filler writes silence.
            return;
        }
        let ch = self.channels as i32;
        let a = r.fs_audio(48_000, ch, count as i32);
        let f = a.f;
        lock(&self.shared.inputs[self.i].status).fs_audio_queue = r.fs_audio_queue_depth();
        if !self.note_audio(&f) || f.no_samples as usize != count {
            return;
        }
        let stride = (f.channel_stride_in_bytes.max(0) as usize) / 4;
        let n_ch = f.no_channels.max(0) as usize;
        if stride < count {
            return;
        }
        self.fs_audio.clear();
        // SAFETY: FLTP, `n_ch` channels of `stride` floats.
        self.fs_audio.extend_from_slice(unsafe {
            std::slice::from_raw_parts(f.p_data as *const f32, stride * n_ch.max(1))
        });
        drop(a);
        let data = std::mem::take(&mut self.fs_audio);
        let skip = (start - s0) as usize;
        self.write_planar(start, &data, stride, n_ch, skip, count - skip);
        self.fs_audio = data;
    }

    fn write_silence(&mut self, start: u64, count: usize) {
        let mut done = 0usize;
        while done < count {
            let c = (count - done).min(1920);
            let end = start + (done + c) as u64;
            if let Err(e) = self.audio.write_samples(end, c, |slices| {
                for (a, b) in slices.iter_mut() {
                    a.fill(0.0);
                    b.fill(0.0);
                }
            }) {
                self.set_error(Some(format!("MXL audio write: {e}")));
                return;
            }
            done += c;
        }
        self.next_sample = Some(start + count as u64);
        lock(&self.shared.inputs[self.i].status).audio_silence += 1;
    }

    /// Fill grains (and audio) nobody wrote.
    fn fill(&mut self) {
        let now = self.clock.now();
        let now_idx = self.clock.index_now(&self.rate);
        // Grain k is filled once TAI passed t(k+1) + half a period.
        let t_now_idx = self.clock.index_to_time(&self.rate, now_idx);
        let due = if now >= t_now_idx + self.period / 2 {
            now_idx.saturating_sub(1)
        } else {
            now_idx.saturating_sub(2)
        };
        if self.last_written + 8 < due {
            self.last_written = due.saturating_sub(1);
        }
        while self.last_written < due {
            let k = self.last_written + 1;
            let hold = self.have_held
                && self.config.in_loss == LossMode::Hold
                && self
                    .last_frame
                    .is_some_and(|t| t.elapsed() < self.config.in_hold);
            self.write_video(k, !hold);
            let mut st = lock(&self.shared.inputs[self.i].status);
            if hold {
                st.repeated += 1;
                if self
                    .last_frame
                    .is_some_and(|t| t.elapsed().as_millis() > 3 * self.period as u128 / 1_000_000)
                {
                    st.state = "holding".into();
                }
            } else if !self.source.is_empty() && st.state != "unsupported" {
                st.state = if self.last_frame.is_some() || st.frames_received > 0 {
                    "no_signal"
                } else {
                    "connecting"
                }
                .into();
            }
        }
        // Audio: silence when none came for 100 ms, up to a grain ago.
        let quiet = self
            .last_audio
            .is_none_or(|t| t.elapsed().as_millis() > 100);
        if quiet {
            let end = self
                .clock
                .time_to_index(&self.arate, now.saturating_sub(self.period));
            let start = match self.next_sample {
                Some(n) if n < end && end - n < 48_000 => n,
                Some(n) if n >= end => return,
                _ => end.saturating_sub(960),
            };
            if end > start {
                self.write_silence(start, (end - start) as usize);
            }
        }
    }

    fn publish(&mut self) {
        let secs = self.sec_start.elapsed().as_secs_f64();
        let s = std::mem::take(&mut self.sec);
        self.sec_start = Instant::now();
        let perf = self
            .receiver
            .as_ref()
            .map(|r| (r.performance(), r.queue(), r.connections()));
        let mut st = lock(&self.shared.inputs[self.i].status);
        st.fps_in = s.frames_in as f64 / secs;
        st.fps_out = s.grains as f64 / secs;
        st.convert_ms = if s.converts > 0 {
            s.convert_ns as f64 / s.converts as f64 / 1e6
        } else {
            0.0
        };
        st.capture_ms = if s.captures > 0 {
            s.capture_ns as f64 / s.captures as f64 / 1e6
        } else {
            0.0
        };
        if s.transit_n > 0 {
            st.transit_ms = Some(s.transit_sum / s.transit_n as f64);
            st.transit_max_ms = Some(s.transit_max);
        } else {
            st.transit_ms = None;
            st.transit_max_ms = None;
        }
        st.latency_ms = (s.latency_n > 0).then(|| s.latency_sum / s.latency_n as f64);
        if let Some(((_total, dropped), q, conns)) = perf {
            st.ndi_dropped_video = dropped.video_frames;
            st.ndi_dropped_audio = dropped.audio_frames;
            st.ndi_queue_video = q.video_frames;
            if conns == 0 && st.state == "connecting" {
                st.sender = None;
            } else if conns > 0 && st.sender.is_none() {
                st.sender = Some(self.source.clone());
            }
        }
    }
}
