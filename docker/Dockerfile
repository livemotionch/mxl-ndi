# syntax=docker/dockerfile:1
# lm-mxl-ndi: MXL ⇄ NDI® bridge.
#   docker build -f docker/Dockerfile --build-arg NDI_SDK_ACCEPT_LICENSE=yes -t lm-mxl-ndi:dev .
#
# NDI_SDK_ACCEPT_LICENSE=yes: you have read and accept the NDI SDK License
# Agreement (https://ndi.link/ndisdk_license; the text is printed during the
# build and shipped in the image under /opt/lm-mxl-ndi/licenses/). The build
# downloads the NDI SDK for Linux (NDI_SDK_URL, always the current release,
# as the licence requires the latest SDK) and keeps from it only the x86-64
# runtime (libndi.so.6) and the licence files; the SDK itself is not in the
# image. NDI® is a registered trademark of Vizrt NDI AB (https://ndi.video/).
#
# MXL pin: dmf-mxl/mxl release/v1.1 218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7, fabrics off
# (the mxl-stack functions' pin).
# UBUNTU: the base image (e.g. public.ecr.aws/docker/library/ubuntu:24.04 when
# Docker Hub rate-limits).

ARG UBUNTU=ubuntu:24.04

FROM ${UBUNTU} AS mxl
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential cmake ninja-build git curl ca-certificates zip unzip tar pkg-config uuid-dev python3 \
    && rm -rf /var/lib/apt/lists/*
COPY scripts/build-mxl.sh /usr/local/bin/build-mxl.sh
RUN VCPKG_ROOT=/opt/vcpkg MXL_BUILD_DIR=/tmp/mxl /usr/local/bin/build-mxl.sh /opt/mxl \
    && rm -rf /tmp/mxl /opt/vcpkg/buildtrees /opt/vcpkg/downloads

# The NDI SDK: download, unpack the installer's payload (what its script
# does after the licence prompt), keep the runtime and licences, build the
# SDK examples used for interop tests and lm-mxl-ndi's ndi-testsrc.
FROM ${UBUNTU} AS ndi
ARG NDI_SDK_URL=https://downloads.ndi.tv/SDK/NDI_SDK_Linux/Install_NDI_SDK_v6_Linux.tar.gz
ARG NDI_SDK_ACCEPT_LICENSE=no
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        curl ca-certificates g++ \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /sdk
RUN curl -fsSL -o sdk.tar.gz "$NDI_SDK_URL" \
    && echo "NDI SDK download: $(sha256sum sdk.tar.gz)" \
    && tar xzf sdk.tar.gz && rm sdk.tar.gz \
    && sh_file=$(ls Install_NDI_SDK_*_Linux.sh) \
    && awk '/^\$view_eula << NDI_EULA_END/{f=1;next}/^NDI_EULA_END/{exit}f' "$sh_file" > /sdk/EULA.txt \
    && if [ "$NDI_SDK_ACCEPT_LICENSE" != yes ]; then \
         cat /sdk/EULA.txt; \
         echo "The NDI SDK License Agreement above must be accepted: --build-arg NDI_SDK_ACCEPT_LICENSE=yes" >&2; \
         exit 1; fi \
    && line=$(awk '/^__NDI_ARCHIVE_BEGIN__/ { print NR+1; exit 0; }' "$sh_file") \
    && tail -n+"$line" "$sh_file" | tar xz && rm "$sh_file" \
    && mv "NDI SDK for Linux" ndi && cat ndi/Version.txt
ENV APP=/opt/lm-mxl-ndi
RUN mkdir -p /out$APP/lib /out$APP/bin /out$APP/licenses /out/tools \
    && lib=$(ls ndi/lib/x86_64-linux-gnu/libndi.so.6.*.*) \
    && cp "$lib" /out$APP/lib/ && ln -s "$(basename "$lib")" /out$APP/lib/libndi.so.6 \
    && cp "ndi/NDI SDK License Agreement.txt" /out$APP/licenses/NDI-SDK-License-Agreement.txt \
    && cp ndi/licenses/libndi_licenses.txt /out$APP/licenses/ \
    && cp ndi/Version.txt /out$APP/licenses/NDI-SDK-Version.txt \
    && cp ndi/bin/x86_64-linux-gnu/ndi-discovery-server /out/tools/
# SDK examples (compiled sample code may ship with the product, see the
# licence's "Bundled Product"), headless, for interop tests:
#   NDIlib_Send_Video_and_Audio  1080p29.97 UYVY + 2 ch, 1000 frames, then exits
#   NDIlib_Send_10bit            1080p29.97 P216 (from v210), 5 min
#   NDIlib_Send_Benchmark        2160p59.94 UYVY, scrolling
#   NDIlib_Recv                  receives the first source found, prints frames
#   NDIlib_Recv_FrameSync_resend receives the first source through a frame-sync and sends it again
#   NDIlib_Find                  lists sources for a minute
# (A source is picked with the SDK's "sourcefilter" regex in ndi-config.v1.json.)
COPY tools/ndi-testsrc.cpp /sdk/tools/
RUN cd ndi/examples/C++ \
    && for e in NDIlib_Send_Video_and_Audio NDIlib_Send_10bit NDIlib_Send_Benchmark NDIlib_Recv NDIlib_Recv_FrameSync_resend NDIlib_Find; do \
         g++ -O2 -std=c++17 -pthread -I/sdk/ndi/include "$e/$e.cpp" -L/out$APP/lib -l:libndi.so.6 -ldl \
             -Wl,-rpath,$APP/lib -o /out$APP/bin/$e || exit 1; done \
    && g++ -O2 -std=c++17 -I/sdk/ndi/include /sdk/tools/ndi-testsrc.cpp -L/out$APP/lib -l:libndi.so.6 \
             -Wl,-rpath,$APP/lib -o /out$APP/bin/ndi-testsrc

FROM ${UBUNTU} AS build
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential curl ca-certificates git pkg-config clang libclang-dev \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.98.1
WORKDIR /src
COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY web ./web
RUN --mount=type=cache,target=/opt/cargo/registry --mount=type=cache,target=/opt/cargo/git \
    cargo build --release --locked --bins \
    && install -m 0755 target/release/lm-mxl-ndi target/release/mxl-compare /usr/local/bin/

# NDI's discovery server, for tests only (an NDI tool: not to be
# redistributed, so not in the bridge image):
#   docker build --target ndi-discovery-test ... -t lm-mxl-ndi:discovery-test
FROM ${UBUNTU} AS ndi-discovery-test
COPY --from=ndi /out/tools/ndi-discovery-server /usr/local/bin/
ENTRYPOINT ["/usr/local/bin/ndi-discovery-server"]

FROM ${UBUNTU} AS runtime
ARG MXL_REF=218ddaa0a08c12ffe75fc475ae65aa3d9eef16d7
ARG VERSION=dev
# libavahi-client: NDI discovery (mDNS) on Linux; dbus + avahi-daemon run
# discovery inside the container (NDI_AVAHI=internal, docker/entrypoint.sh).
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        ca-certificates libstdc++6 libavahi-client3 libavahi-common3 avahi-daemon dbus util-linux \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /config && chown 1000:1000 /config \
    && sed -i -e 's/^#*\s*enable-dbus=.*/enable-dbus=yes/' -e '/^rlimit-/d' /etc/avahi/avahi-daemon.conf
COPY --from=mxl /opt/mxl/lib/libmxl.so* /usr/local/lib/
# The NDI runtime stays in the application's folder (as the SDK asks), found
# through NDI_RUNTIME_DIR_V6.
COPY --from=ndi /out/opt/lm-mxl-ndi/ /opt/lm-mxl-ndi/
COPY --from=build /usr/local/bin/lm-mxl-ndi /usr/local/bin/mxl-compare /usr/local/bin/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
COPY docker/NOTICE /opt/lm-mxl-ndi/licenses/NOTICE
RUN ldconfig && ldd /usr/local/lib/libmxl.so.1 | (! grep -q "not found") \
    && ldd /opt/lm-mxl-ndi/lib/libndi.so.6 | (! grep -q "not found") \
    && ldd /usr/local/bin/lm-mxl-ndi | (! grep -q "not found") \
    && for b in /opt/lm-mxl-ndi/bin/*; do ldd "$b" | (! grep -q "not found") || exit 1; done
ENV NDI_RUNTIME_DIR_V6=/opt/lm-mxl-ndi/lib NDI_STATE_DIR=/config NDI_CONFIG_DIR=/config/ndi \
    PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin:/opt/lm-mxl-ndi/bin
# Web/API/metrics, NMOS. NDI: TCP 5960 (source queries), 5961 and up (one
# per stream), UDP 5960 and up (reliable UDP, one per process); mDNS 5353.
EXPOSE 8210 3335
LABEL org.opencontainers.image.title="lm-mxl-ndi" \
      org.opencontainers.image.description="MXL <-> NDI bridge (Livemotion). NDI is a registered trademark of Vizrt NDI AB, https://ndi.video/" \
      org.opencontainers.image.version="${VERSION}" \
      io.dmf.mxl.revision="${MXL_REF}"
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["/usr/local/bin/lm-mxl-ndi"]
