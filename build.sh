#!/bin/bash
# Build script for RuSStly
# Requires libasound2-dev (or equivalent) for audio support

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Set up ALSA headers if locally extracted (for environments without libasound2-dev)
if [ -d /tmp/opencode/alsa-local ]; then
    export PKG_CONFIG_PATH=/tmp/opencode/alsa-local/usr/lib/x86_64-linux-gnu/pkgconfig:$PKG_CONFIG_PATH
    export C_INCLUDE_PATH=/tmp/opencode/alsa-local/usr/include:$C_INCLUDE_PATH
    export CPLUS_INCLUDE_PATH=/tmp/opencode/alsa-local/usr/include:$CPLUS_INCLUDE_PATH
    export LIBRARY_PATH=/tmp/opencode/alsa-local/usr/lib/x86_64-linux-gnu:$LIBRARY_PATH
fi

cargo build "$@"
