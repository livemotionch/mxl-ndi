#!/usr/bin/env bash
# Copy this checkout to the mxl-test VM, build lm-mxl-ndi:dev there, restart the
# test instance (~/lm-mxl-ndi-run, compose project lm-ndi).
# Building downloads the NDI SDK and accepts its licence (NDI_SDK_ACCEPT_LICENSE=yes).
set -euo pipefail
host="${1:?usage: vm-redeploy.sh <user@vm>}"
cd "$(dirname "$0")/.."
rsync -a --delete --exclude target --exclude .git ./ "$host:lm-mxl-ndi/"
ssh "$host" 'cd ~/lm-mxl-ndi && docker build -q --build-arg UBUNTU=public.ecr.aws/docker/library/ubuntu:24.04 --build-arg NDI_SDK_ACCEPT_LICENSE=yes -f docker/Dockerfile -t lm-mxl-ndi:dev . \
  && cd ~/lm-mxl-ndi-run && docker compose -p lm-ndi up -d --force-recreate 2>&1 | tail -1'
