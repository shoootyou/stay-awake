#!/usr/bin/env bash
set -euo pipefail

SUDO=()
if [ "$(id -u)" -ne 0 ]; then
  SUDO=(sudo)
fi

packages=(
  build-essential
  libgtk-3-dev
  libwebkit2gtk-4.1-dev
  libayatana-appindicator3-dev
  librsvg2-dev
  patchelf
)

"${SUDO[@]}" apt-get update
"${SUDO[@]}" apt-get install -y "${packages[@]}"
