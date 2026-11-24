//! MXL → NDI: one thread per output, one NDI sender each (not clocked by
//! the SDK). Per grain `N` of the house rate, at TAI `t(N)`, it reads the
//! video grain of `t(N − NDI_OUT_READ_OFFSET_GRAINS)` (a writer finishes
//! grain `M` shortly after `t(M)`), converts v210 to UYVY or P216, and sends
//! it with that grain's TAI time as the NDI `timecode` (100 ns units), then
//! the audio samples of the same grain period (FLTP), with the same
//! timecode. A receiver (this bridge's NDI input) can so keep A/V sync and
//! measure the latency. (NDI's `timestamp` is set by the SDK itself, to the
//! UTC submit time.)
//!
//! The MXL side is the vision mixer's input reader (`reader.rs`): a stalled
//! or re-created flow holds the last grain for `NDI_OUT_HOLD_MS`, then black.
//! Without a route the output sends black and silence (`NDI_OUT_IDLE_BLACK`),
//! so the NDI source stays up. Without receivers it sends nothing (the SDK
//! would compress every frame for no one).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::config::{Config, OutPixel};
use crate::convert;
use crate::mxlio::{self, Api, AudioReader, Clock};
use crate::ndi::{self, fourcc, NdiApi};
use crate::reader::{Instances, Route, VideoInput};
use crate::state::{lock, Shared};

pub fn labels(config: &Config, i: usize) -> (String, String) {
    (
        format!("{} Out {} Video", config.label, i + 1),
        format!("{} Out {} Audio", config.label, i + 1),
    )
}

#[derive(Default)]
struct Second {
    frames: u64,
    convert_ns: u64,
    send_ns: u64,
    audio_send_ns: u64,
    audio_frames: u64,
}

pub struct Output {
    i: usize,
    config: Arc<Config>,
    shared: Arc<Shared>,
    api: Api,
    clock: Clock,
    sender: ndi::Sender,
    rate: mxl_sys::Rational,
    arate: mxl_sys::Rational,
    period: u64,
    video: VideoInput,
    instances: Instances,
    audio_route: Option<Route>,
    audio: Option<AudioReader>,
    audio_retry: u64,
    audio_buf: Vec<Vec<f32>>,
    planar_audio: Vec<f32>,
    frame_buf: Vec<u8>,
    black: Vec<u8>,
    black_size: (usize, usize),
    revision: u64,
    connections: i32,
    sec: Second,
    sec_start: Instant,
}

impl Output {
    pub fn new(
        api: &Api,
        ndi: &NdiApi,
        config: Arc<Config>,
        shared: Arc<Shared>,
        i: usize,
    ) -> Result<Output, String> {
        let name = config.outputs[i].name.clone();
        let sender = ndi::Sender::new(ndi, &name, config.ndi_groups.as_deref())?;
        let address = sender.source_name();
        tracing::info!("out-{}: NDI sender {address:?}", i + 1);
        {
            let mut st = lock(&shared.outputs[i].status);
            st.index = i + 1;
            st.name = name;
            st.address = address;
            st.state = "no_route".into();
        }
        let f = config.format;
        Ok(Output {
            i,
            rate: mxlio::rational(f.rate_n, f.rate_d),
            arate: mxlio::rational(48_000, 1),
            period: f.frame_ns(),
            config,
            shared,
            api: api.clone(),
            clock: Clock::new(api.clone()),
            sender,
            video: VideoInput::new(format!("out-{}", i + 1)),
            instances: Instances::default(),
            audio_route: None,
            audio: None,
            audio_retry: 0,
            audio_buf: Vec::new(),
            planar_audio: Vec::new(),
            frame_buf: Vec::new(),
            black: Vec::new(),
            black_size: (0, 0),
            revision: u64::MAX,
            connections: 0,
            sec: Second::default(),
            sec_start: Instant::now(),
        })
    }

    fn routes(&mut self) {
        let slot = &self.shared.outputs[self.i];
        let rev = slot.revision.load(Ordering::Acquire);
        if rev == self.revision {
            return;
        }
        self.revision = rev;
        let v = lock(&slot.video_route).clone();
        let a = lock(&slot.audio_route).clone();
        self.video.set_route(v.clone());
        if a != self.audio_route {
            self.audio_route = a.clone();
            self.audio = None;
            self.audio_retry = 0;
        }
        let mut st = lock(&slot.status);
        st.video_route = v;
        st.audio_route = a;
    }

    pub fn run(mut self) {
        let mut n = self.clock.index_now(&self.rate) + 1;
        while !self.shared.stopping() {
            let t = self.clock.index_to_time(&self.rate, n);
            self.clock.sleep_until(t + 500_000);
            let now = self.clock.now();
            if now > t + 2 * self.period {
                // Fell behind (a stall): skip to now.
                let behind = self.clock.index_now(&self.rate);
                lock(&self.shared.outputs[self.i].status).skipped += behind - n;
                n = behind;
                continue;
            }
            self.routes();
            // Nobody connected: keep the reader current, send nothing
            // (the SDK would compress every frame for no one).
            if self.connections == 0 {
                self.video
                    .poll(&self.api, &mut self.instances, &self.clock, now);
                lock(&self.shared.outputs[self.i].status).state = "idle".into();
                n += 1;
                if self.sec_start.elapsed().as_secs_f64() >= 1.0 {
                    self.publish();
                }
                continue;
            }
            let src_idx = n.saturating_sub(self.config.out_read_offset);
            let src_t = self.clock.index_to_time(&self.rate, src_idx);
            self.video_frame(src_t, now);
            self.audio_frame(src_t);
            n += 1;
            if self.sec_start.elapsed().as_secs_f64() >= 1.0 {
                self.publish();
            }
        }
    }

    fn video_frame(&mut self, src_t: u64, now: u64) {
        let (w, h, out_codec) = (
            self.config.format.width as usize,
            self.config.format.height as usize,
            match self.config.out_pixel {
                OutPixel::Uyvy => fourcc::UYVY,
                OutPixel::P216 => fourcc::P216,
            },
        );
        let frame = self.video.frame(
            &self.api,
            &mut self.instances,
            &self.clock,
            src_t,
            now,
            0,
            self.config.out_hold_max.as_nanos() as u64,
        );
        let t0 = Instant::now();
        let (fw, fh) = match &frame {
            Some(f) => (f.width, f.height),
            None => (w, h),
        };
        let bpp = if out_codec == fourcc::P216 { 4 } else { 2 };
        let size = fw * fh * bpp;
        let data: &[u8] = match &frame {
            Some(f) => {
                // SAFETY: the reader stays open while the grain is used.
                let g = unsafe { f.grain.data() };
                if g.len() < convert::v210_frame_bytes(fw, fh) {
                    return;
                }
                self.frame_buf.resize(size, 0);
                if out_codec == fourcc::P216 {
                    convert::v210_to_p216(g, fw, fh, &mut self.frame_buf);
                } else {
                    convert::v210_to_uyvy(g, fw, fh, &mut self.frame_buf);
                }
                &self.frame_buf
            }
            None => {
                let mut st = lock(&self.shared.outputs[self.i].status);
                st.black += 1;
                drop(st);
                if !self.config.out_idle_black {
                    return;
                }
                if self.black_size != (fw, fh) || self.black.len() != size {
                    let mut v = vec![0u8; convert::v210_frame_bytes(fw, fh)];
                    convert::v210_black(fw, fh, &mut v);
                    self.black.resize(size, 0);
                    if out_codec == fourcc::P216 {
                        convert::v210_to_p216(&v, fw, fh, &mut self.black);
                    } else {
                        convert::v210_to_uyvy(&v, fw, fh, &mut self.black);
                    }
                    self.black_size = (fw, fh);
                }
                &self.black
            }
        };
        let t1 = Instant::now();
        let f = self.config.format;
        // UYVY: 2 bytes a pixel; P216: a 16-bit Y plane and a CbCr plane,
        // both with a stride of 2 bytes a pixel.
        let vf = ndi::VideoFrame {
            xres: fw as i32,
            yres: fh as i32,
            fourcc: out_codec,
            frame_rate_n: f.rate_n as i32,
            frame_rate_d: f.rate_d as i32,
            picture_aspect_ratio: fw as f32 / fh.max(1) as f32,
            frame_format_type: ndi::frame_format::PROGRESSIVE,
            timecode: (src_t / 100) as i64,
            p_data: data.as_ptr() as *mut u8,
            line_stride_in_bytes: (fw * 2) as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        debug_assert!(data.len() >= size);
        self.sender.send_video(&vf);
        self.sec.convert_ns += (t1 - t0).as_nanos() as u64;
        self.sec.send_ns += t1.elapsed().as_nanos() as u64;
        self.sec.frames += 1;
        let mut st = lock(&self.shared.outputs[self.i].status);
        st.frames_sent += 1;
        st.state = serde_json::to_value(self.video.state)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        st.error = self.video.error.clone();
        st.late = self.video.counters.late;
        st.held = self.video.counters.held;
    }

    fn audio_frame(&mut self, src_t: u64) {
        let now = self.clock.now();
        if self.audio.is_none() && now >= self.audio_retry {
            if let Some(route) = self.audio_route.clone() {
                self.audio_retry = now + 1_000_000_000;
                let r = self
                    .instances
                    .get(&self.api, &route.domain)
                    .and_then(|i| i.audio_reader(route.flow).map_err(|e| e.to_string()));
                let mut st = lock(&self.shared.outputs[self.i].status);
                match r {
                    Ok(r) if r.rate.numerator == 48_000 && r.rate.denominator == 1 => {
                        tracing::info!(
                            "out-{}: reading audio {} ({} channels)",
                            self.i + 1,
                            route.flow,
                            r.channels
                        );
                        self.audio = Some(r);
                        st.audio_state = "ok".into();
                    }
                    Ok(r) => {
                        st.audio_state =
                            format!("unsupported: {} Hz (48 kHz only)", r.rate.numerator)
                    }
                    Err(e) => st.audio_state = format!("waiting: {e}"),
                }
            } else {
                lock(&self.shared.outputs[self.i].status).audio_state = "no_route".into();
            }
        }
        let s0 = self.clock.time_to_index(&self.arate, src_t);
        let s1 = self.clock.time_to_index(&self.arate, src_t + self.period);
        let count = (s1 - s0) as usize;
        if count == 0 || count > 48_000 {
            return;
        }
        let channels = self
            .audio
            .as_ref()
            .map_or(self.config.audio_channels, |r| r.channels)
            .max(1);
        self.audio_buf.resize(channels, Vec::new());
        let mut ok = false;
        if let Some(r) = &self.audio {
            // The writer may finish this period's samples a little after
            // the video grain: wait for them up to 5 ms.
            for attempt in 0..6 {
                match r.read(s1, count, &mut self.audio_buf) {
                    Ok(true) => {
                        ok = true;
                        break;
                    }
                    Ok(false) => {
                        if attempt < 5 {
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    Err(e) => {
                        lock(&self.shared.outputs[self.i].status).audio_state =
                            format!("error: {e}");
                        self.audio = None;
                        break;
                    }
                }
            }
            if !ok {
                lock(&self.shared.outputs[self.i].status).audio_gaps += 1;
            }
        } else if !self.config.out_idle_black {
            return;
        }
        self.planar_audio.clear();
        self.planar_audio.resize(channels * count, 0.0);
        if ok {
            for (c, b) in self.audio_buf.iter().enumerate() {
                let n = b.len().min(count);
                self.planar_audio[c * count..c * count + n].copy_from_slice(&b[..n]);
            }
        }
        let af = ndi::AudioFrame {
            sample_rate: 48_000,
            no_channels: channels as i32,
            no_samples: count as i32,
            timecode: (src_t / 100) as i64,
            fourcc: fourcc::FLTP,
            p_data: self.planar_audio.as_mut_ptr() as *mut u8,
            channel_stride_in_bytes: (count * 4) as i32,
            p_metadata: std::ptr::null(),
            timestamp: 0,
        };
        let t0 = Instant::now();
        self.sender.send_audio(&af);
        self.sec.audio_send_ns += t0.elapsed().as_nanos() as u64;
        self.sec.audio_frames += 1;
        lock(&self.shared.outputs[self.i].status).audio_frames += 1;
    }

    fn publish(&mut self) {
        let secs = self.sec_start.elapsed().as_secs_f64();
        let s = std::mem::take(&mut self.sec);
        self.sec_start = Instant::now();
        let mut st = lock(&self.shared.outputs[self.i].status);
        st.fps = s.frames as f64 / secs;
        if s.frames > 0 {
            st.convert_ms = s.convert_ns as f64 / s.frames as f64 / 1e6;
            st.send_ms = s.send_ns as f64 / s.frames as f64 / 1e6;
        }
        self.connections = self.sender.connections();
        st.connections = self.connections;
        if self.connections == 0 {
            st.fps = 0.0;
        }
        if s.audio_frames > 0 {
            st.audio_send_ms = s.audio_send_ns as f64 / s.audio_frames as f64 / 1e6;
        }
        if st.address.is_empty() {
            st.address = self.sender.source_name();
        }
    }
}
