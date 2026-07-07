#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

echo "==> Building RuSStly"
echo "    Linux: install libasound2-dev (or alsa-lib-devel) before building"
echo ""

cargo build --release "$@"

echo ""
echo "==> Binary: target/release/russtly"
