// ndi-testsrc: a headless NDI(R) test sender for interop tests of lm-mxl-ndi,
// written against the NDI SDK's C API (NDI(R) is a registered trademark of
// Vizrt NDI AB, https://ndi.video/). Independent of the bridge's code.
//
//   ndi-testsrc [--name N] [--size WxH] [--rate N/D] [--bits 8|16]
//               [--channels C] [--frames F] [--no-audio] [--yuv file.uyvy]
//
// Picture: 75% colour bars (BT.709, limited range) over the top two thirds,
// a grey ramp below, and a white vertical bar moving one step per frame.
// --yuv: a raw UYVY picture of --size instead (8-bit), with the moving bar.
// Audio: a 1 kHz sine at the SMPTE reference level (+/-1.0 is -20 dBFS, so
// 0.1 is -40 dBFS), FLTP 48 kHz, one block per frame. The SDK clocks the
// video (clock_video = true); timecodes are synthesised.
#include <Processing.NDI.Lib.h>

#include <cmath>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

static volatile bool stop = false;
static void on_signal(int) { stop = true; }

int main(int argc, char** argv) {
    std::string name = "Test";
    int w = 1920, h = 1080, rn = 50, rd = 1, bits = 8, channels = 2;
    long frames = 0;
    bool audio = true;
    std::string yuv;
    for (int i = 1; i < argc; i++) {
        std::string a = argv[i];
        auto next = [&]() -> const char* { return i + 1 < argc ? argv[++i] : ""; };
        if (a == "--name") name = next();
        else if (a == "--size") std::sscanf(next(), "%dx%d", &w, &h);
        else if (a == "--rate") { if (std::sscanf(next(), "%d/%d", &rn, &rd) < 2) rd = 1; }
        else if (a == "--bits") bits = std::atoi(next());
        else if (a == "--channels") channels = std::atoi(next());
        else if (a == "--frames") frames = std::atol(next());
        else if (a == "--no-audio") audio = false;
        else if (a == "--yuv") yuv = next();
        else { std::fprintf(stderr, "unknown option %s\n", a.c_str()); return 2; }
    }
    std::signal(SIGINT, on_signal);
    std::signal(SIGTERM, on_signal);
    if (!NDIlib_initialize()) { std::fprintf(stderr, "NDI cannot run on this CPU\n"); return 1; }
    NDIlib_send_create_t sc;
    sc.p_ndi_name = name.c_str();
    sc.clock_video = true;
    sc.clock_audio = false;
    NDIlib_send_instance_t s = NDIlib_send_create(&sc);
    if (!s) { std::fprintf(stderr, "NDIlib_send_create failed\n"); return 1; }

    // 10-bit limited-range YCbCr of 75% bars: white yellow cyan green magenta red blue.
    const int bars[7][3] = {{721, 512, 512}, {646, 176, 539}, {525, 591, 176}, {450, 255, 203},
                            {335, 769, 821}, {260, 448, 848}, {139, 848, 485}};
    const bool p216 = bits > 8;
    const int stride = 2 * w;  // UYVY: 2 bytes/pixel; P216: 2 bytes/sample per plane
    std::vector<uint8_t> base(p216 ? stride * h * 2 : stride * h), frame(base.size());
    auto put = [&](std::vector<uint8_t>& buf, int x, int y, int Y, int C) {
        // C: the chroma sample of this pixel (Cb on even x, Cr on odd x)
        if (p216) {
            uint16_t* yp = (uint16_t*)buf.data();
            uint16_t* cp = (uint16_t*)(buf.data() + stride * h);
            yp[y * w + x] = (uint16_t)(Y << 6);
            cp[y * w + x] = (uint16_t)(C << 6);
        } else {
            buf[y * stride + 2 * x] = (uint8_t)((C + 2) >> 2);
            buf[y * stride + 2 * x + 1] = (uint8_t)((Y + 2) >> 2);
        }
    };
    for (int y = 0; y < h; y++)
        for (int x = 0; x < w; x++) {
            int Y, Cb, Cr;
            if (y < 2 * h / 3) {
                const int* b = bars[x * 7 / w];
                Y = b[0]; Cb = b[1]; Cr = b[2];
            } else {
                Y = 64 + (876 * x) / (w - 1); Cb = Cr = 512;
            }
            put(base, x, y, Y, (x & 1) ? Cr : Cb);
        }

    if (!yuv.empty()) {
        if (p216) { std::fprintf(stderr, "--yuv is 8-bit UYVY\n"); return 2; }
        FILE* f = std::fopen(yuv.c_str(), "rb");
        if (!f || std::fread(base.data(), 1, base.size(), f) != base.size()) {
            std::fprintf(stderr, "cannot read %zu bytes of %s\n", base.size(), yuv.c_str());
            return 2;
        }
        std::fclose(f);
    }
    NDIlib_video_frame_v2_t v;
    v.xres = w; v.yres = h;
    v.FourCC = p216 ? NDIlib_FourCC_video_type_P216 : NDIlib_FourCC_video_type_UYVY;
    v.frame_rate_N = rn; v.frame_rate_D = rd;
    v.picture_aspect_ratio = (float)w / (float)h;
    v.frame_format_type = NDIlib_frame_format_type_progressive;
    v.line_stride_in_bytes = stride;
    v.p_data = frame.data();

    std::vector<float> pcm;
    double phase = 0, samples_acc = 0;
    std::printf("ndi-testsrc: %s %dx%d @ %d/%d, %s, %d ch audio%s\n", name.c_str(), w, h, rn, rd,
                p216 ? "P216" : "UYVY", channels, audio ? "" : " (off)");
    std::fflush(stdout);
    long n = 0;
    for (; !stop && (frames == 0 || n < frames); n++) {
        frame = base;
        int bx = (int)((n * 8) % w) & ~1;
        for (int y = 0; y < 2 * h / 3; y++)
            for (int x = bx; x < bx + 16 && x < w; x++) put(frame, x, y, 940, 512);
        NDIlib_send_send_video_v2(s, &v);
        if (audio && channels > 0) {
            samples_acc += 48000.0 * rd / rn;
            int count = (int)samples_acc;
            samples_acc -= count;
            pcm.assign((size_t)count * channels, 0.f);
            for (int k = 0; k < count; k++) {
                float val = (float)(0.1 * std::sin(phase));
                phase += 2 * M_PI * 1000.0 / 48000.0;
                for (int c = 0; c < channels; c++) pcm[(size_t)c * count + k] = val;
            }
            NDIlib_audio_frame_v3_t a;
            a.sample_rate = 48000; a.no_channels = channels; a.no_samples = count;
            a.FourCC = NDIlib_FourCC_audio_type_FLTP;
            a.p_data = (uint8_t*)pcm.data();
            a.channel_stride_in_bytes = count * (int)sizeof(float);
            NDIlib_send_send_audio_v3(s, &a);
        }
        if (n % (rn / rd * 10 + 1) == 0) {
            std::printf("frame %ld, %d connection(s)\n", n, NDIlib_send_get_no_connections(s, 0));
            std::fflush(stdout);
        }
    }
    std::printf("ndi-testsrc: sent %ld frames\n", n);
    NDIlib_send_destroy(s);
    NDIlib_destroy();
    return 0;
}
