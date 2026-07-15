#!/usr/bin/env sh
set -eu
cd "$(dirname "$0")"
# The Linux build intentionally contains only winit's X11 backend. Keeping the
# toolkit hint explicit also prevents helper UI from preferring Wayland.
export GDK_BACKEND=x11
exec cargo run --release
