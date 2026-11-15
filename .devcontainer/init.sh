#!/usr/bin/env bash

# Installs cosign, for `directory scan` to verify images' signatures, from Sigstore's own release
# on GitHub: its Debian package for the container's architecture, checked against the checksum the
# release published, recorded here so a changed asset fails rather than installs.
#
# The version is the one .github/workflows/scan.yaml installs; bump both together, with the
# checksums from the release's cosign_checksums.txt.

set -euo pipefail

version="3.1.4"
declare -A checksums=(
  [amd64]="6db74c0ad58e69fca184bf0c6c4be40304aa12ab81b4b672042f91979934d547"
  [arm64]="ccd63f2710d7fd4becd9f3db93b4bec15e154893fe7aa8260136575092398561"
)

if command -v cosign >/dev/null && cosign version 2>/dev/null | grep -q "v${version}$"; then
  echo "cosign v${version} is installed"
  exit 0
fi

arch="$(dpkg --print-architecture)"
checksum="${checksums[$arch]:-}"
if [ -z "${checksum}" ]; then
  echo "no cosign checksum recorded for ${arch}, see https://github.com/sigstore/cosign/releases/tag/v${version}" >&2
  exit 1
fi

deb="cosign_${version}_${arch}.deb"
dir="$(mktemp -d)"
trap 'rm -rf "${dir}"' EXIT
curl --fail --silent --show-error --location --output "${dir}/${deb}" \
  "https://github.com/sigstore/cosign/releases/download/v${version}/${deb}"
echo "${checksum}  ${dir}/${deb}" | sha256sum --check --quiet
sudo dpkg --install "${dir}/${deb}"
cosign version | grep GitVersion
