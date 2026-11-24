//! lm-mxl-ndi: a bridge between MXL (Media eXchange Layer) and NDI®, with an
//! NMOS IS-04/IS-05 node for MXL routing. NDI® is a registered trademark of
//! Vizrt NDI AB (https://ndi.video/).

pub mod api;
pub mod config;
pub mod convert;
pub mod input;
pub mod mxlio;
pub mod ndi;
pub mod nmos;
pub mod output;
pub mod reader;
#[cfg(target_arch = "x86_64")]
pub mod simd;
pub mod state;
