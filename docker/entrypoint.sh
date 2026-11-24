#!/bin/sh
# NDI discovery on Linux is mDNS through avahi-daemon (D-Bus). NDI_AVAHI:
#   internal  start dbus and avahi-daemon in the container (as root), then run
#             the command as RUN_AS (default 1000:1000). Needs host
#             networking for mDNS to reach the LAN.
#   host      use the host's avahi-daemon: mount /run/dbus (and
#             /run/avahi-daemon) from the host.
#   off       no mDNS: set NDI_DISCOVERY_SERVER (an NDI discovery server)
#             and/or NDI_EXTRA_IPS, or give inputs ndi://ip:port.
set -eu
mode="${NDI_AVAHI:-internal}"
run_as="${RUN_AS:-1000:1000}"
cfg="${NDI_CONFIG_DIR:-/config/ndi}"
if [ "$(id -u)" = 0 ]; then
    mkdir -p "$cfg"
    chown "$run_as" "${NDI_STATE_DIR:-/config}" "$cfg" 2>/dev/null || true
    if [ "$mode" = internal ]; then
        mkdir -p /run/dbus /run/avahi-daemon
        rm -f /run/dbus/pid /run/avahi-daemon/pid
        dbus-daemon --system --fork
        # shellcheck disable=SC2086
        avahi-daemon --daemonize --no-chroot ${AVAHI_ARGS:-}
        for _ in 1 2 3 4 5 6 7 8 9 10; do avahi-daemon --check 2>/dev/null && break; sleep 0.3; done
    fi
    exec setpriv --reuid="${run_as%:*}" --regid="${run_as#*:}" --clear-groups -- "$@"
fi
mkdir -p "$cfg" 2>/dev/null || true
exec "$@"
