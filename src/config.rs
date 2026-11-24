//! Configuration from the environment (the mxl-stack conventions; see the
//! README's table). Invalid values are an error: the process exits with 78.

use std::path::PathBuf;
use std::time::Duration;

use uuid::Uuid;

use crate::mxlio::flowdef::VideoFormat;

/// What an NDI input writes while its source is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LossMode {
    /// The last frame for `NDI_IN_HOLD_MS`, then black
    Hold,
    /// Black at once
    Black,
}

/// How an NDI input places frames on the MXL grain clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncMode {
    /// The bridge's own TAI placement: sender time + measured delay
    Tai,
    /// NDI's frame-sync (time-base corrector, audio resampling), pulled once
    /// per grain
    Framesync,
}

/// The uncompressed format handed to NDI for sending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutPixel {
    /// 8-bit 4:2:2 (v210 rounded to 8 bits)
    Uyvy,
    /// 16-bit semi-planar 4:2:2: all 10 bits (NDI's high-bit-depth SpeedHQ)
    P216,
}

#[derive(Debug, Clone)]
pub struct InputConfig {
    /// NDI source (`HOST (Name)`, `ndi://ip:port` or `HOST (Name)@ip:port`);
    /// empty: not connected
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct OutputConfig {
    /// NDI source name (the address becomes `HOST (name)`)
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub format: VideoFormat,
    pub inputs: Vec<InputConfig>,
    pub outputs: Vec<OutputConfig>,
    /// Prefix of every NMOS label of this instance (`NDI` → `NDI In 1 Video`)
    pub label: String,
    pub nmos_label: String,
    pub nmos_port: u16,
    pub web_port: u16,
    pub output_domain_dir: PathBuf,
    pub scan_path: PathBuf,
    pub output_domain_id: Uuid,
    pub seed: String,
    pub cleanup_on_exit: bool,
    pub history_ns: u64,
    pub state_dir: PathBuf,
    pub shutdown_timeout: Duration,
    pub audio_channels: usize,
    // NDI in
    pub in_loss: LossMode,
    pub in_hold: Duration,
    /// Added to the delay every input picks at the first frame
    pub in_delay_ns: u64,
    pub in_scale: bool,
    pub in_sync: SyncMode,
    /// `NDIlib_recv_color_format_e` of the inputs (best or fastest)
    pub in_color: i32,
    /// `NDIlib_recv_bandwidth_e` of the inputs (highest or lowest)
    pub in_bandwidth: i32,
    // NDI out
    pub out_pixel: OutPixel,
    pub out_read_offset: u64,
    pub out_hold_max: Duration,
    /// Send black and silence while an output has no route or signal
    pub out_idle_black: bool,
    // NDI SDK
    /// Discovery server(s), `host[:port]`, comma separated
    pub ndi_discovery_server: Option<String>,
    /// Extra IPs to query for sources (no mDNS), comma separated
    pub ndi_extra_ips: Option<String>,
    /// NDI groups of the senders and receivers (default: Public)
    pub ndi_groups: Option<String>,
    /// Where `ndi-config.v1.json` is written (`NDI_CONFIG_DIR` of the SDK)
    pub ndi_config_dir: PathBuf,
    /// Write `ndi-config.v1.json` from the settings above (false: leave a
    /// mounted file alone)
    pub ndi_config_write: bool,
    pub log_level: String,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match var(name) {
        None => Ok(default),
        Some(v) => v.parse().map_err(|_| format!("{name}={v:?} is not valid")),
    }
}

fn boolean(name: &str, default: bool) -> Result<bool, String> {
    match var(name).as_deref().map(str::to_ascii_lowercase).as_deref() {
        None => Ok(default),
        Some("1" | "true" | "yes" | "on") => Ok(true),
        Some("0" | "false" | "no" | "off") => Ok(false),
        Some(v) => Err(format!("{name}={v:?}: expected true or false")),
    }
}

fn ranged(name: &str, default: usize, lo: usize, hi: usize) -> Result<usize, String> {
    let v: usize = parse(name, default)?;
    if v < lo || v > hi {
        return Err(format!("{name}={v}: must be {lo}–{hi}"));
    }
    Ok(v)
}

impl Config {
    pub fn from_env() -> Result<Config, String> {
        let format = VideoFormat::parse(&var("VIDEO_FORMAT").unwrap_or_else(|| "1080p50".into()))?;
        let scan_path =
            PathBuf::from(var("MXL_DOMAIN_SCAN_PATH").unwrap_or_else(|| "/Volumes/mxl".into()));
        let output_domain_dir = PathBuf::from(
            var("MXL_OUTPUT_DOMAIN_DIR")
                .unwrap_or_else(|| scan_path.join("ndi-bridge").to_string_lossy().into()),
        );
        let seed = var("NMOS_SEED").unwrap_or_else(|| "lm-mxl-ndi".into());
        let output_domain_id = mxl_nmos::ids::derive_id(&seed, "mxl-output-domain");
        let n_in = ranged("NDI_INPUTS", 1, 0, 32)?;
        let n_out = ranged("NDI_OUTPUTS", 1, 0, 32)?;
        if n_in + n_out == 0 {
            return Err("NDI_INPUTS and NDI_OUTPUTS are both 0".into());
        }
        let label = var("NDI_LABEL_PREFIX").unwrap_or_else(|| "NDI".into());
        let inputs = (1..=n_in)
            .map(|i| InputConfig {
                source: var(&format!("NDI_IN{i}_SOURCE")).unwrap_or_default(),
            })
            .collect();
        let outputs = (1..=n_out)
            .map(|i| OutputConfig {
                name: var(&format!("NDI_OUT{i}_NAME"))
                    .unwrap_or_else(|| format!("{label} Out {i}")),
            })
            .collect::<Vec<_>>();
        for (i, a) in outputs.iter().enumerate() {
            if outputs[..i].iter().any(|b| b.name == a.name) {
                return Err(format!("NDI_OUT{}_NAME {:?} is used twice", i + 1, a.name));
            }
        }
        let in_loss = match var("NDI_IN_LOSS").as_deref() {
            None | Some("hold") => LossMode::Hold,
            Some("black") => LossMode::Black,
            Some(v) => return Err(format!("NDI_IN_LOSS={v:?}: expected hold or black")),
        };
        let out_pixel = match var("NDI_OUT_PIXEL")
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("uyvy") => OutPixel::Uyvy,
            Some("p216") => OutPixel::P216,
            Some(v) => return Err(format!("NDI_OUT_PIXEL={v:?}: expected uyvy or p216")),
        };
        let in_sync = match var("NDI_IN_SYNC")
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("tai") => SyncMode::Tai,
            Some("framesync") => SyncMode::Framesync,
            Some(v) => return Err(format!("NDI_IN_SYNC={v:?}: expected tai or framesync")),
        };
        let in_color = match var("NDI_IN_COLOR")
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("best") => crate::ndi::color_format::BEST,
            Some("fastest") => crate::ndi::color_format::FASTEST,
            Some(v) => return Err(format!("NDI_IN_COLOR={v:?}: expected best or fastest")),
        };
        let in_bandwidth = match var("NDI_IN_BANDWIDTH")
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            None | Some("highest") => crate::ndi::bandwidth::HIGHEST,
            Some("lowest") => crate::ndi::bandwidth::LOWEST,
            Some(v) => {
                return Err(format!(
                    "NDI_IN_BANDWIDTH={v:?}: expected highest or lowest"
                ))
            }
        };
        let state_dir = PathBuf::from(var("NDI_STATE_DIR").unwrap_or_else(|| "/config".into()));
        Ok(Config {
            format,
            inputs,
            outputs,
            nmos_label: var("NMOS_LABEL").unwrap_or_else(|| "NDI bridge".into()),
            label,
            nmos_port: parse("NMOS_PORT", 3335)?,
            web_port: parse("WEB_PORT", 8210)?,
            output_domain_dir,
            scan_path,
            output_domain_id,
            seed,
            cleanup_on_exit: boolean("MXL_CLEANUP_ON_EXIT", false)?,
            history_ns: parse::<u64>("MXL_HISTORY_MS", 200)?.clamp(40, 10_000) * 1_000_000,
            ndi_config_dir: var("NDI_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| state_dir.join("ndi")),
            state_dir,
            shutdown_timeout: Duration::from_secs(parse("SHUTDOWN_TIMEOUT_S", 10)?),
            audio_channels: ranged("AUDIO_CHANNELS", 2, 1, 32)?,
            in_loss,
            in_hold: Duration::from_millis(parse("NDI_IN_HOLD_MS", 1000)?),
            in_delay_ns: parse::<u64>("NDI_IN_DELAY_MS", 0)? * 1_000_000,
            in_scale: boolean("NDI_IN_SCALE", true)?,
            in_sync,
            in_color,
            in_bandwidth,
            out_pixel,
            out_read_offset: parse("NDI_OUT_READ_OFFSET_GRAINS", 1)?,
            out_hold_max: Duration::from_millis(parse("NDI_OUT_HOLD_MS", 1000)?),
            out_idle_black: boolean("NDI_OUT_IDLE_BLACK", true)?,
            ndi_discovery_server: var("NDI_DISCOVERY_SERVER"),
            ndi_extra_ips: var("NDI_EXTRA_IPS"),
            ndi_groups: var("NDI_GROUPS"),
            ndi_config_write: boolean("NDI_CONFIG_WRITE", true)?,
            log_level: var("LOG_LEVEL").unwrap_or_else(|| "info".into()),
        })
    }

    /// The NMOS node's settings (`NMOS_*`).
    pub fn nmos_settings(&self) -> Result<mxl_nmos::Settings, String> {
        let mut s = mxl_nmos::Settings::new(self.nmos_label.clone(), self.nmos_port);
        s.description = "MXL ⇄ NDI® bridge (lm-mxl-ndi)".into();
        s.service_name_prefix = "lm-mxl-ndi".into();
        s.apply_env().map_err(|e| e.to_string())?;
        if s.seed.is_none() {
            s.seed = Some(self.seed.clone());
        }
        Ok(s)
    }
}
