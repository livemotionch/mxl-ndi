# Test results: mxl-test VM, 2026-11-24

VM `mxl-test` (Infomaniak a16-ram32: 16 vCPU AMD EPYC, no GPU), Ubuntu 24.04, next to
mxl-stack's single-host example (multiviewer, replay, WebRTC monitor, lm-mxl-mixer,
lm-mxl-omt, ...), which alone keeps the host at about 40% of 16 vCPU. lm-mxl-ndi
`lm-mxl-ndi:dev` built on the VM with NDI SDK 6.4.0.2 (2026-11-17), 8 NDI inputs and 8
NDI outputs, 1080p50, `NDI_IN_SYNC=tai`, `NDI_IN_COLOR=best`, `NDI_OUT_PIXEL=uyvy` unless
noted. All NDI traffic stays on the host (loopback; the SDK uses one TCP connection per
stream there). Numbers from the bridge's `/api/status` (`scripts/vm/`: `scale.py`,
`soak.py`, `avs.sh`), mxl-avsync 2.2.0, `mxl-compare` (code-value error between two MXL
flows) and nftables counters on NDI's ports (`ndibw.sh`, the SDK has no byte counters).
NDI® is a registered trademark of Vizrt NDI AB (https://ndi.video/).

## 1. Loopback: test player → NDI out → NDI in → MXL

Test player `Out 1` (MXL) → `NDI Out 1` → NDI (SpeedHQ, TCP) → `NDI In 1` → multiviewer
`MV In 10`.

| | |
|---|---|
| Frames | 50.0 fps in and out, 0 dropped / repeated / black, 0 late at the output |
| End-to-end latency, MXL grain time to MXL grain time | **1 grain (20 ms)**: source grain N comes out as grain N+1 (`mxl-compare` offset 1); some inputs settle at 2 grains (see §2) |
| Of which NDI (output read → NDI input arrival, `transit`) | 25–26 ms mean, 27 ms max per second: the 20 ms output read offset plus ~6 ms of convert, SpeedHQ encode, TCP, decode |
| Time base | `timecode` (the output's TAI grain time), so placement is exact |
| A/V sync (mxl-avsync generator → NDI Out 3 → NDI In 3 → analyzer, 120 events) | **−0.008 ms**, identical to the generator routed straight into the analyzer (−0.008 ms). Stddev 0, 0 resyncs |
| Picture, 8-bit (UYVY out), test card | mean luma error 0.93, chroma 0.50 (10-bit code values); 98.3% / 98.9% of samples within ±2 |
| Picture, 8-bit, mixer programme (VM PGM) | luma 0.98, chroma 0.52; 97% within ±2, 0.01% > 16 (codec errors at sharp edges) |
| Picture, 10-bit (P216 out, `best` in), VM PGM | **luma 0.18, chroma 0.15; 93.3% / 92.7% of samples exact** |
| Codec time per frame | send (SpeedHQ encode in the call) 2.1 ms UYVY, 3.1–3.7 ms P216; decode runs in the SDK's `ndir:video` threads (~7% of a core per stream) |
| Conversion per frame | v210 → UYVY 0.45 ms (AVX2), v210 → P216 2.1 ms, P216 → v210 0.9 ms (rounded), UYVY → v210 0.65 ms |
| Bandwidth, test card + 16 ch audio | **49 Mbit/s** per stream (on the wire incl. TCP/IP headers) |
| Bandwidth, real picture (the photo of lm-mxl-omt's tests, `ndi-testsrc --yuv`, 1080p60 + 2 ch) | **127 Mbit/s** |

## 2. Scale: N streams each way (N outputs and N inputs, looped)

30 s windows after 20 s of settling. CPU in % of one core. Video sources: test player
Out 1/2 and VM PGM.

| Streams each way | Bridge CPU | Host CPU (16 vCPU) | Input thread | Output thread | Dropped / repeated / late / audio gaps | Latency | Transit mean / max | Video per stream |
|---|---|---|---|---|---|---|---|---|
| 1 | 53% | 44.2% | 6% | 12% | 0 / 0 / 0 / 0 | 20 ms | 26.3 / 27.8 ms | 49 Mbit/s (incl. 16 ch audio) |
| 4 | 131% | 50.7% | 8% | 12% | 0 / 0 / 0 / 0 | 20 ms | 26.9 / 29.7 ms | |
| 8 | 237% | 57.6% | 8% | 12% | 0 / 0 / 0 / 0 | 20 ms | 27.8 / 32.4 ms | 397 Mbit/s for 8 |
| 8, P216 (10-bit) | 348% | 62.9% | 7% | 24% | 0 / 0 / 0 / 0 | 20–40 ms¹ | 30.6 / 44.0 ms | |
| 8, `NDI_IN_COLOR=fastest` | 203% | 54.5% | 5% | 11% | 0 / 0 / 0 / 0 | 20 ms² | 26.9 / 32.4 ms | |
| 8, P216 out, `fastest` in | 321% | 60.9% | 5% | 24% | 0 / 0 / 0 / 0 | 20 ms² | 29.9 / 36.4 ms | |
| 8, `NDI_IN_SYNC=framesync` | 246% | | 7% | | 0 / 0 / 0 / 0 | 20–40 ms | – | |

So about **30% of one core per 1080p50 stream pair** (one in, one out) in 8-bit, 43% in
10-bit. The in-/out- threads carry only part of it: the SDK decodes in its own
`ndir:video` threads. Idle inputs (black and silence written) cost ~2% each; outputs
without receivers send nothing.

¹ With P216 and load, frames of some inputs sometimes take > 40 ms (the slowest frame of a
2 s window decides), so their delay stays at 2 grains — as lm-mxl-omt.
² Input 1 (fed from the mixer programme) at 40 ms, the others at 20 ms.

**Choice of the receive format.** `best` gives P216 for every SpeedHQ source, `fastest`
UYVY. Accuracy on the same 10-bit path: `best` 0.18 / 93% exact, `fastest` 1.33 / 37%
exact (cut to 8 bits). On the 8-bit path both are equal (0.98 vs 0.96). `best` costs
+34% of a core at 8 inputs (~4% per input). Default: `best`.

The first batch of these runs showed inputs 2–8 at ±2 ms latency with ~30 dropped and
repeated frames a second: a reconnecting input first got the sender's cached last frame,
seconds old, and fell back to the SDK `timestamp`. Fixed (stale frames are skipped and
don't decide the time base); the table is the run after the fix.

## 3. Interop with independent NDI endpoints

The SDK's examples (built in the image from the SDK's sample code) and `ndi-testsrc` (this
repo, C++ on the SDK's API, not the bridge's code), run headless in the bridge's container
(sharing its avahi-daemon). Examples that connect to "the first source" got an
`NDI_CONFIG_DIR` with a `sourcefilter` regex.

- **NDI Out 5 → `NDIlib_Recv`**: 1204 video frames (1920×1080) and 1204 audio frames
  (960 samples) in 24 s: 50 fps, A/V interleaved.
- **NDI Out 5 → `NDIlib_Recv_FrameSync_resend` → `MXL-TEST (NDIlib_Recv_FrameSync_resend)`
  → NDI In 6**: the example receives through NDI's frame-sync and sends again (clocked, 4 ch
  audio). In 30 s: 1501 frames, 0 dropped / repeated / resyncs / audio resyncs. Its
  timecode passes through, so NDI In 6 reports the whole chain: **80 ms** (transit 68 ms).
- **60p source, `ndi-testsrc` 1080p60 + 2 ch → NDI In 5** (written as 1080p50): `tai`:
  312 dropped, 12 repeated, 1 resync, 2 audio resyncs in 30 s (placed by the SDK
  `timestamp`; the SDK-clocked sender's submit jitter adds a few repeats). `framesync`:
  exactly 300 dropped (10/s), 0 repeated, 0 resyncs, 0 audio resyncs.
- **Other size, live**: `ndi-testsrc` 1280×720p60 → scaled to 1080p50: correct picture,
  319 dropped / 19 repeated in 30 s, but the bilinear scaler takes **14.5 ms a frame** (53%
  of a core). `NDIlib_Send_Benchmark` 3840×2160p59.94 (it sends unclocked, ~257 fps):
  before the UHD fast path the scaler took 30.7 ms a frame, the input reached only 37 fps
  (511 repeats in 30 s); with the 2×2 path **8 ms a frame on the loaded VM** (3.5 ms
  alone), 1500 grains in 30 s, 0 repeated, the excess frames dropped before conversion.
- **10-bit source, `NDIlib_Send_10bit`** (P216, 1080p29.97): received as P216, 900 frames
  in 30 s, 20 repeats a second (29.97 → 50), 0 dropped.
- **Source loss and reconnect**: `NDIlib_Send_Video_and_Audio` (1080p29.97 UYVY + 2 ch)
  stops after 1000 frames. NDI In 4 went `ok` → `holding` (31.5 s) → black and silence
  exactly 1.0 s later (`NDI_IN_HOLD_MS`), the flow kept running at 50 fps. Restarted, it
  was back (`ok`) after **1.4 s**, including the process start.
- **Discovery without mDNS** (`NDI_AVAHI=off`): no discovery: 0 sources, the input stays
  `connecting`. `NDI_DISCOVERY_SERVER=127.0.0.1` (NDI's discovery server 6.4, test-only
  image): 8 sources registered and found, NDI In 2 ok at 50 fps. `NDI_EXTRA_IPS=127.0.0.1`:
  8 sources found (queried on TCP 5960), NDI In 2 ok.

Ports seen with 8 outputs: TCP 5960 (source query), 5961–5968 (one per output); no UDP
5960+ on loopback (reliable UDP is for other hosts). mDNS on 5353 shared with lm-mxl-omt's
avahi-daemon without conflict.

## 4. NDI ⇄ OMT through MXL, NMOS routing

- **NDI → OMT**: test player Out 1 → NDI Out 6 → NDI In 6 → `OMT Out 7` → OMT In 7: 3
  grains end to end (NDI hop 20 ms, OMT hop 40 ms), picture mean luma error 0.92 (two
  8-bit codecs ≈ one: the 10 → 8 bit rounding dominates).
- **OMT → NDI**: test player Out 1 → `OMT Out 8` → OMT In 8 → NDI Out 7 → NDI In 7: 2
  grains (40 ms), mean luma error 0.92, A/V and timing intact (NDI In 7 by timecode).
  (OMT Out/In 7–8 were idle; cleared and unrouted afterwards.)
- **Into the multiviewer**: `NDI In 1 → MV In 10` (video and audio).
- **Into lm-mxl-mixer**: `NDI In 5 → VM In 12` (a free input), shown on ME 2 preview for
  10 s: `lm_mixer_input_ok{input="in-12"}` 1, 0 late / held / missed frames. (ME 2 preview
  and the route restored afterwards.)
- **Mixer PGM → NDI**: `VM PGM Video/Audio → NDI Out 1` → NDI In 1 → MV In 10 (left
  running).
- **Restart** of the bridge (many times, `--force-recreate`): IS-05 routes and input
  sources restored, inputs back within seconds.

## 5. NDI vs OMT bridge

Same VM, same load, 1080p50, numbers from §1–4 and lm-mxl-omt's docs/test-results.md.

| | lm-mxl-ndi (NDI 6.4, SpeedHQ) | lm-mxl-omt (libomt, VMX) |
|---|---|---|
| CPU, 8 + 8 streams, 8-bit | 237% (`best`), 203% (`fastest`) | 202% |
| CPU per stream pair, 8-bit | ~30% (`best`), ~25% (`fastest`) | ~25% |
| CPU, 8 + 8, 10-bit (P216) | 348% | 345% |
| Soak 8 + 8, 10 min | 0 dropped / repeated / late, 232% avg, 275% max | 0 dropped / repeated / late, 208% avg, 244% max |
| End-to-end latency (loopback) | 1 grain (20 ms), some inputs 2 grains | 1 grain (20 ms), some inputs 2 grains |
| Transit (incl. 20 ms read offset) | 26–28 ms mean | 25–26 ms mean |
| Encode / decode per frame | 2.1 ms send (encode in the call) / decode in SDK threads | 1.4 ms / 1.0 ms |
| A/V sync through the bridge | −0.008 ms (generator direct: −0.008 ms) | −0.008 ms |
| Bandwidth, test card (+16 ch audio) | 49 Mbit/s in total | 39 Mbit/s video + 24.6 Mbit/s audio = 64 Mbit/s |
| Bandwidth, photo 1080p60 | 127 Mbit/s (+2 ch) | 205 Mbit/s |
| Picture, 8-bit, same source (VM PGM), mean luma / chroma error | 0.98 / 0.52 | 0.95 / 0.50 |
| Picture, 10-bit | P216 path: mean 0.18, 93% of samples exact (lossy SpeedHQ at high bit depth) | P216 path: identical to the source on the test card |
| 10-bit sources received | as P216 (`best`), all 10 bits kept | as P216 |
| Free-running sender at 60p | `tai`: 10 drops/s + a few repeats; `framesync`: exactly 10/s, audio resampled | 10 drops/s, 0 repeats |
| Source loss: hold, then black; reconnect | 1.0 s hold; back 1.4 s after restart | 1.0 s hold; back 1.2 s after restart |
| Discovery without mDNS | discovery server, extra IPs (both tested), `ndi://ip:port` | OMT discovery server, `omt://host:port` |

In short: on this VM the two bridges are equivalent in timing and A/V sync, NDI costs
~20% more CPU at 8-bit with `best` (the same with `fastest`) and the same at 10-bit, uses
less bandwidth (49 vs 64 Mbit/s with 16 ch audio, 127 vs 205 Mbit/s for the photo; NDI's audio share was not measured separately), and is slightly less
exact at 10-bit (SpeedHQ is lossy at every bit depth, VMX at high bit depth was lossless
on the test card).

## Soak: 8 + 8 streams, 10 minutes

All 8 outputs looped into the 8 inputs (VM PGM, mxl-avsync generator, test player),
1080p50, UYVY, `best`, `tai`; mxl-avsync analyzer on NDI In 3 for the whole run:

| | |
|---|---|
| Grains written by the inputs | 240,080 (8 × 50 × 600 = 240,000) |
| Dropped / repeated / black / resyncs / audio resyncs / silence | 0 / 0 / 0 / 0 / 0 / 0 |
| Frames sent by the outputs | 240,080; late 0, held 0, skipped 0, audio gaps 0 |
| A/V sync (1000 events) | −0.008 ms, min = max, stddev 0 |
| Bridge CPU | 232% of one core on average, 275% at most |
| Host CPU | 58.7% of 16 vCPU (stack alone ≈ 40%) |
| NDI traffic | 424 Mbit/s for 8 streams (incl. 16 ch audio each) |
| Latency per input | 20 ms for 5 inputs, 40 ms for 3; transit max 34 ms in the last second |
