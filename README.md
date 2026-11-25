# lm-mxl-ndi

A bridge between **MXL** (the Media eXchange Layer shared-memory data plane of
the EBU / Linux Foundation Dynamic Media Facility) and **NDI®**
([ndi.video](https://ndi.video/)), the NDI counterpart of
[lm-mxl-omt](../lm-mxl-omt) (the MXL ⇄ OMT bridge), which is modelled on
[mxl-decklink](https://github.com/LeeO86/mxl-decklink). It runs next to the
[mxl-stack](https://github.com/LeeO86/mxl-stack) functions and is routed with
NMOS IS-04/IS-05 like them.

NDI® is a registered trademark of Vizrt NDI AB. lm-mxl-ndi is not a product of
Vizrt NDI AB and is not endorsed by it.

**Where it came from.** The code was copied from lm-mxl-omt at commit
`4275943` (2026-11-24) and adapted, not shared as a crate: the MXL reader and
writer (`src/mxlio/`, `src/reader.rs`), the v210 conversions and AVX2 kernels
(`src/convert.rs`, `src/simd.rs`), the NMOS node (`src/nmos.rs`), TAI frame
placement, hold-then-black, the status page and API (`src/api.rs`,
`web/index.html`), the entrypoint with avahi, and the VM test scripts. New
here: `src/ndi.rs` (the NDI SDK, loaded at run time), the NDI input and
output (`src/input.rs`, `src/output.rs`: timecode/timestamp time bases, NDI's
frame-sync as an option), the UHD → HD and rounded P216 conversions,
`mxl-compare` (picture accuracy) and `tools/ndi-testsrc.cpp`.

- **NDI in → MXL.** Each NDI input connects to an NDI source (by name, found
  by mDNS, a discovery server or extra IPs; or `ndi://ip:port`) and writes a
  `video/v210` flow in the house format (e.g. 1080p50) and an `audio/float32`
  48 kHz flow, TAI-locked. The flows always exist: black and silence without a
  source.
- **MXL → NDI out.** Each NDI output reads an MXL video and audio flow (routed
  with IS-05) and sends them as an NDI source (`HOST (NDI Out 1)`): video as
  UYVY (8-bit) or P216 (10-bit), audio as planar float (FLTP).
- **NMOS** (BCP-007-03, `mxl-nmos`): one node and device; per NDI input an
  IS-04 video and audio **sender** (`NDI In 1 Video`, `NDI In 1 Audio`), per
  NDI output an IS-05 video and audio **receiver** (`NDI Out 1 Video`, ...).
  Routes and input sources are kept across restarts.
- Status page and JSON (`/api/status`), `GET /api/sources`,
  `PUT /api/inputs/<n>`, `/metrics` for Prometheus — as lm-mxl-omt.

Rust; the NDI SDK's C API, loaded with `dlopen` and `NDIlib_v5_load()`.

## How it works

```
 NDI source ──SpeedHQ──► SDK receive (decode, own threads) ──P216/UYVY──► v210 ──► MXL video flow (grain = ts + D)
                                                          └──FLTP──────────────► MXL audio flow (sample = ts + D)

 MXL video flow ──v210──► UYVY or P216 ──► SDK send (SpeedHQ, in the call) ──► NDI receivers
 MXL audio flow ──float32──► FLTP ───────┘  timecode = the grain's TAI time
```

**Formats.** v210 ⇄ UYVY is a repack and a shift; P216 (16-bit 4:2:2, a Y
plane then a CbCr plane) carries all 10 bits and NDI compresses it as
high-bit-depth SpeedHQ (`NDI_OUT_PIXEL=p216`). Inputs ask the SDK for
`NDIlib_recv_color_format_best` (`NDI_IN_COLOR=best`). The SDK then hands out
**P216 for every SpeedHQ source**, 8-bit or 10-bit, and UYVY only for 8-bit
codecs (H.264). `fastest` would always give UYVY, cutting 10-bit sources to
8 bits. Measured (docs/test-results.md): on a 10-bit path `best` is 7× more
accurate (mean code-value error 0.18 vs 1.33, 93% of samples exact); on an
8-bit path both are equal; `best` costs about 4% of a core more per stream
(P216 → v210 is 0.9 ms a frame). So `best` is the default and `fastest` an
option for 8-bit-only systems. P216 is rounded (not truncated) to 10 bits.

A source of another size is scaled: exactly twice the house size (UHD → HD)
by a 2×2 average (3.5 ms a frame), any other size bilinear (10-bit planar;
`NDI_IN_SCALE=false` refuses it). Another frame rate is frame-synchronised
(frames dropped or repeated, counted). Single fields (the SDK always delivers
fields for interlaced sources with `best`/`fastest`) are scaled to the full
height (bob). Audio must be 48 kHz; channels beyond `AUDIO_CHANNELS` are
dropped, missing ones silent.

**Timing (NDI in, `NDI_IN_SYNC=tai`, default).** NDI frames carry the sender's
`timecode` (this bridge's outputs put the grain's TAI time there) and the
`timestamp` the sending SDK sets to the UTC time of submission. An input
places frames by the timecode when it is a plausible TAI time, else by the
timestamp (+ the TAI−UTC offset), else by arrival. At the first frame it picks
a delay `D` = arrival − sender time + `NDI_IN_DELAY_MS`; a frame of sender
time `ts` goes to the grain whose period holds `ts + D`, its audio to the
sample index of `ts + D`. So A/V keep the sync they had at the sender and
network jitter doesn't move frames between grains. `D` is picked again (a
*resync*) when frames keep arriving a grain late, when a 2 s window shows it
could be a grain shorter, or when the sender's clock jumps. A new connection
to an idle sender first gets that sender's last frame, seconds old: it is
skipped (`stale`) and doesn't decide the time base.

**Timing (`NDI_IN_SYNC=framesync`).** NDI's own frame-sync
(`NDIlib_framesync_*`: a time-base corrector with dynamic audio resampling)
is pulled once per grain, 1 ms after the grain starts, for one video frame and
exactly one grain of audio. It absorbs a free-running sender's clock drift
smoothly (resampled audio, no audio resyncs), but it doesn't know the TAI
grain clock: on the VM its audio came out **38 ms after the video** (its audio
queue holds ~35 ms), against −0.008 ms with `tai`. Hence `tai` is the default;
`framesync` is for free-running senders whose audio drifts, where a constant
A/V offset is acceptable (or corrected downstream).

**Timing (NDI out).** Per grain `N`, at TAI `t(N)`, the output reads the grain
of `N − NDI_OUT_READ_OFFSET_GRAINS` (default 1), sends it (synchronously: the
SDK compresses in the call, ~2 ms UYVY, ~3–4 ms P216) with that grain's TAI
time as timecode, then that grain period's audio. The sender is not clocked by
the SDK. Another lm-mxl-ndi receiving it reports the true end-to-end latency
(`latency_ms`) and the network/codec part (`transit_ms`). Outputs without
receivers send nothing.

**Source loss (NDI in).** Grains nobody wrote are filled half a grain after
they are due: with the last frame for `NDI_IN_HOLD_MS` (`NDI_IN_LOSS=hold`),
then black; silence when no audio came for 100 ms. The SDK reconnects by
itself; when the source is back the input picks its delay again.

**Stalled MXL flows (NDI out).** As lm-mxl-omt: a writer that stalls or
re-creates its flow holds the last grain for `NDI_OUT_HOLD_MS`, then black; no
route sends black and silence (`NDI_OUT_IDLE_BLACK=false`: nothing).

**Discovery.** NDI on Linux uses mDNS through avahi-daemon (D-Bus). The
image's entrypoint starts dbus and avahi-daemon inside the container
(`NDI_AVAHI=internal`; the container starts as root and runs the bridge as
`RUN_AS`), so it needs host networking. It coexists with lm-mxl-omt's own
avahi-daemon on the same host. With a host avahi-daemon: `NDI_AVAHI=host` and
mount `/run/dbus`. Without mDNS (clouds that drop multicast, hosts on other
subnets): `NDI_DISCOVERY_SERVER=host[:port],...` (an NDI Discovery Server;
senders then register there and skip mDNS), and/or
`NDI_EXTRA_IPS=ip,...` (hosts whose sources are queried directly on TCP 5960),
or inputs with `ndi://ip:port`. Both are written to `ndi-config.v1.json` in
`NDI_CONFIG_DIR` (default `/config/ndi`) before the SDK starts, which the SDK
reads (`NDI_CONFIG_WRITE=false` keeps a mounted file instead); extra IPs also
go to the finder of `/api/sources`.

**Ports** (host network; measured on the VM): TCP **5960** (each host's
source query service), TCP **5961 and up** (one per NDI output: 5961–5968
for 8 outputs), UDP **5960 and up** for reliable UDP between hosts (one per
process), TCP/UDP 6960+ / 7960+ only in the non-default multi-TCP / UDP
modes, UDP 5353 (mDNS), TCP 5959 (a discovery server). All clear of OMT's
6400–6431 as long as fewer than ~440 NDI streams run. On one host (loopback)
the SDK uses its TCP connection per stream.

## Configuration

| Variable | Default | |
|---|---|---|
| `VIDEO_FORMAT` | `1080p50` | House format of the input flows (720/1080/2160, p23.98…p60) |
| `NDI_INPUTS` / `NDI_OUTPUTS` | 1 / 1 | Number of NDI inputs and outputs (0–32) |
| `NDI_IN<n>_SOURCE` | – | Source of input n: `HOST (Name)`, `ndi://ip:port` or `HOST (Name)@ip:port`. Also set at run time in the UI / `PUT /api/inputs/<n>` (saved in `NDI_STATE_DIR/sources.json`; the environment wins at start) |
| `NDI_OUT<n>_NAME` | `<prefix> Out <n>` | NDI source name of output n (the full name is `HOST (name)`) |
| `NDI_LABEL_PREFIX` | `NDI` | NMOS labels: `NDI In 1 Video`, `NDI Out 1 Audio`, ...; receiver names `NDI In 1` |
| `AUDIO_CHANNELS` | 2 | Channels of the input audio flows |
| `NDI_IN_SYNC` | `tai` | `tai` (sender time + measured delay) or `framesync` (NDI's frame-sync) |
| `NDI_IN_COLOR` | `best` | `best` (P216 from SpeedHQ, keeps 10 bits) or `fastest` (UYVY) |
| `NDI_IN_BANDWIDTH` | `highest` | `highest` or `lowest` (the SDK's low-bandwidth stream) |
| `NDI_IN_LOSS` | `hold` | `hold` (last frame for `NDI_IN_HOLD_MS`, then black) or `black` |
| `NDI_IN_HOLD_MS` | 1000 | |
| `NDI_IN_DELAY_MS` | 0 | Extra delay on top of the measured one (more jitter headroom) |
| `NDI_IN_SCALE` | true | Scale sources of another size; false: refuse them |
| `NDI_OUT_PIXEL` | `uyvy` | `uyvy` (8-bit) or `p216` (10-bit) |
| `NDI_OUT_READ_OFFSET_GRAINS` | 1 | How many grains behind now the outputs read |
| `NDI_OUT_HOLD_MS` | 1000 | Hold a stalled MXL flow this long, then black |
| `NDI_OUT_IDLE_BLACK` | true | Send black and silence without a route |
| `NDI_AVAHI` | `internal` | Entrypoint: `internal`, `host` or `off` (see Discovery) |
| `NDI_DISCOVERY_SERVER` | – | NDI discovery server(s), `host[:port]`, comma separated |
| `NDI_EXTRA_IPS` | – | Hosts to query for sources without mDNS, comma separated |
| `NDI_GROUPS` | – (Public) | NDI groups of the outputs and the finder |
| `NDI_CONFIG_DIR` | `/config/ndi` | Where `ndi-config.v1.json` is written (the SDK's `NDI_CONFIG_DIR`) |
| `NDI_CONFIG_WRITE` | true | Write `ndi-config.v1.json` from the settings above |
| `NDI_RUNTIME_DIR_V6` | `/opt/lm-mxl-ndi/lib` (image) | Folder of `libndi.so.6` (the SDK's convention) |
| `WEB_PORT` / `NMOS_PORT` | 8210 / 3335 | |
| `NMOS_*` | | As every mxl-nmos node (registry, host address, seed, label) |
| `MXL_DOMAIN_SCAN_PATH` | `/Volumes/mxl` | Domains the receivers may read |
| `MXL_OUTPUT_DOMAIN_DIR` | `<scan>/ndi-bridge` | The input flows' domain |
| `MXL_HISTORY_MS` | 200 | Ring length of a new output domain |
| `MXL_CLEANUP_ON_EXIT` | false | Remove the output domain on exit |
| `NDI_STATE_DIR` | `/config` | Sources and IS-05 connections, kept across restarts |

Exit codes: 0, 75 (port, libmxl or the NDI runtime unavailable), 78
(configuration), 143 (SIGTERM).

## Build and run

```bash
docker build -f docker/Dockerfile --build-arg NDI_SDK_ACCEPT_LICENSE=yes -t lm-mxl-ndi:dev .
# Docker Hub rate limit: --build-arg UBUNTU=public.ecr.aws/docker/library/ubuntu:24.04
```

`NDI_SDK_ACCEPT_LICENSE=yes` states that whoever builds has read and accepts
the NDI SDK License Agreement; without it the build prints the agreement and
stops. The build downloads the current NDI SDK for Linux
(`NDI_SDK_URL`, `Install_NDI_SDK_v6_Linux.tar.gz`; its sha256 and version are
printed), unpacks the installer's payload without running it, and keeps only
the x86-64 runtime `libndi.so.6` (in `/opt/lm-mxl-ndi/lib`, the application's
folder, as the SDK asks; not the system path) and the licence files. The image
also has the SDK examples `NDIlib_Send_Video_and_Audio`, `NDIlib_Send_10bit`,
`NDIlib_Send_Benchmark`, `NDIlib_Recv`, `NDIlib_Recv_FrameSync_resend`,
`NDIlib_Find` and this repo's `ndi-testsrc` (headless senders and receivers
for interop tests), and `mxl-compare`. The NDI discovery server is an NDI tool
and not in the image; `--target ndi-discovery-test` builds a separate local
image with it, for tests.

Rebuild with `--no-cache` (or a new `NDI_SDK_URL`) from time to time: the
licence requires the latest SDK, and Docker would otherwise keep the cached
download.

Next to mxl-stack's single-host compose: `deploy/compose.single-host.yaml`;
on the mxl-test VM: `deploy/compose.mxl-test.yaml`, `scripts/vm-redeploy.sh`.

Local development: `cargo test` (conversions, FFI layout, addresses, config
file) needs neither libmxl nor the NDI SDK. With the SDK unpacked somewhere:
`NDI_RUNTIME_DIR_V6=<sdk>/lib/x86_64-linux-gnu cargo test --lib smoke -- --ignored`
sends and receives UYVY, P216 and FLTP frames in one process.

Test tools in the image: `ndi-testsrc --size 1280x720 --rate 60/1 --bits 16
--channels 2 [--yuv picture.uyvy]`; `mxl-compare <domain A> <flow A> <domain
B> <flow B> [grains]` (code-value error between two flows, grain offset found
automatically). SDK examples connect to the first source they find: give them
an `NDI_CONFIG_DIR` whose `ndi-config.v1.json` has
`{"ndi":{"sourcefilter":{"regex":"MXL-TEST \\(NDI Out 5\\)"}}}`.

## Licences and attributions

lm-mxl-ndi is © 2026 Livemotion, released under the [MIT licence](LICENSE). The
MIT licence covers this repository's code only, not the NDI SDK. It uses:

- **The NDI® SDK for Linux** (NDI 6, `libndi.so.6`, © 2023-2026 Vizrt NDI AB),
  under the NDI SDK License Agreement (https://ndi.link/ndisdk_license). The
  image ships the runtime and compiled SDK sample programs, unmodified, with
  the agreement, `libndi_licenses.txt` (the runtime's third-party notices:
  RapidJSON, SpeexDSP, ...), the SDK version and a NOTICE, all in
  `/opt/lm-mxl-ndi/licenses/`. The SDK itself (headers, docs, tools) is not
  in the image and not in this repository; the build downloads it. The SDK's
  headers are MIT; `src/ndi.rs` follows them.
- **MXL** (dmf-mxl/mxl, Apache-2.0), loaded at run time; `mxl` / `mxl-sys`
  Rust bindings (Apache-2.0). **mxl-nmos** (LeeO86, MIT / Apache-2.0).
- `src/mxlio/domain.rs` and `scripts/build-mxl.sh` come from LeeO86's
  mxl-audio-mixer (MIT) through [lm-mxl-mixer](https://github.com/livemotionch/lm-mxl-mixer) and
  [mxl-omt](https://github.com/livemotionch/mxl-omt); `src/mxlio/`, `src/reader.rs`
  and `src/simd.rs` from lm-mxl-mixer through mxl-omt (both MIT).
- Structure and conventions follow LeeO86's mxl-decklink (MIT); no code from it.

What the NDI licence asks of this bridge, and how it is met:

- **Trademark notice and link.** "NDI® is a registered trademark of Vizrt NDI
  AB" with a link to https://ndi.video/ near where NDI is used: on the status
  page (footer, and a link at the NDI inputs where sources are chosen), in
  this README, the image's NOTICE and label, and the `src/ndi.rs` header.
- **Redistribution.** The runtime and compiled sample code may ship inside the
  product ("Bundled Product"); the SDK and the NDI tools (discovery server,
  etc.) may not be redistributed — hence the separate `ndi-discovery-test`
  target. No image is published; build your own.
- **Latest SDK.** The licence requires products to use the latest SDK: the
  build always downloads the current release (no pinned version or checksum).
- **No modification or reverse engineering** of the SDK; it is loaded as is.

Points to check before any wider use (not legal advice):

- The licence's definition of "Products" excludes software running on a
  "fixed-purpose virtual or cloud environment … containerized instance …"
  that restricts the user from changing the OS or installing software; that
  use needs a commercial licence from NDI. A container on machines whose
  users can change the OS and install software reads as outside this
  exclusion; a locked-down appliance or hosted service built on this image
  would not.
- Using "NDI" in a product's name falls under the NDI brand guidelines.
- Distributing the bridge to third parties would need an EULA with the terms
  of the licence's §3d.

## Test results and known gaps

Measured on the mxl-test VM: [docs/test-results.md](docs/test-results.md),
with the comparison against lm-mxl-omt. In short, 1080p50 through NDI and back
adds one grain (20 ms) with A/V sync unchanged (−0.008 ms), 8 streams each way
ran with no dropped or late frames at ~30% of a core per stream pair (8-bit;
OMT: 25%), the SDK's own example sender, receiver and frame-sync re-sender
interoperate, a discovery server and extra IPs work without mDNS, and NDI ⇄
OMT through MXL works both ways.

Known gaps:

- Only on one host so far: all NDI traffic was loopback TCP. Reliable UDP
  between hosts, mDNS across hosts and the multicast modes are untested.
- The bilinear scaler is scalar: 720p → 1080p costs 14.5 ms a frame (half a
  core per input). UHD → HD has a fast path (3.5 ms).
- `framesync` mode: audio ~38 ms after video (the frame-sync's audio queue);
  not compensated.
- `tai` mode with a free-running (SDK-clocked) sender at another rate: the
  submit-time jitter costs a few extra repeats and an occasional resync
  (12 repeats, 1 resync, 2 audio resyncs in 30 s of 60p → 50p).
- Interlaced sources: single fields are scaled to full height (bob); no
  weaving or field-accurate output.
- Audio: 48 kHz only; no resampling in `tai` mode (a drifting sender's audio
  is absorbed by a 10 ms slip, then a resync).
- No NDI metadata, tally, PTZ, HDR metadata (the standard SDK can't send it),
  alpha (UYVA/PA16 alpha is dropped) or 4:4:4.
- No byte counters in the SDK: NDI bandwidth is not on the status page
  (measured with `scripts/vm/ndibw.sh`, nftables).
- As lm-mxl-omt: the latency trim keeps the delay over the slowest frame of
  2 s, so a single slow frame can keep an input at 2 grains (40 ms).
- NDI discovery over mDNS needs an avahi-daemon; the image runs one in the
  container as root before dropping to `RUN_AS`.
