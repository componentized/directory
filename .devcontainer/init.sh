#!/usr/bin/env bash

# Installs the tools the dev container needs beyond its image, each skipped when it's installed.

set -euo pipefail

# cosign, for `directory scan` to verify images' signatures, from Sigstore's own release on GitHub:
# its Debian package for the container's architecture, checked against the checksum the release
# published, recorded here so a changed asset fails rather than installs.
#
# The version is the one .github/workflows/scan.yaml installs; bump both together, with the
# checksums from the release's cosign_checksums.txt.
install_cosign() {
  local version="3.1.4"
  local -A checksums=(
    [amd64]="6db74c0ad58e69fca184bf0c6c4be40304aa12ab81b4b672042f91979934d547"
    [arm64]="ccd63f2710d7fd4becd9f3db93b4bec15e154893fe7aa8260136575092398561"
  )

  if command -v cosign >/dev/null && cosign version 2>/dev/null | grep -q "v${version}$"; then
    echo "cosign v${version} is installed"
    return
  fi

  local arch checksum
  arch="$(dpkg --print-architecture)"
  checksum="${checksums[$arch]:-}"
  if [ -z "${checksum}" ]; then
    echo "no cosign checksum recorded for ${arch}, see https://github.com/sigstore/cosign/releases/tag/v${version}" >&2
    return 1
  fi

  local deb="cosign_${version}_${arch}.deb"
  local dir
  dir="$(mktemp -d)"
  # expanded now, as `dir` is gone by the time the script exits
  # shellcheck disable=SC2064
  trap "rm -rf '${dir}'" EXIT
  curl --fail --silent --show-error --location --output "${dir}/${deb}" \
    "https://github.com/sigstore/cosign/releases/download/v${version}/${deb}"
  echo "${checksum}  ${dir}/${deb}" | sha256sum --check --quiet
  sudo dpkg --install "${dir}/${deb}"
  cosign version | grep GitVersion
}

# A headless Chromium, `chromium-headless-shell`, to load the built site in and check its pages
# render and behave, e.g. with Playwright, given its path as the browser to launch. Debian's own
# package, which brings the libraries it needs and has the distribution's security updates.
install_headless_shell() {
  if command -v chromium-headless-shell >/dev/null; then
    echo "$(chromium-headless-shell --version) is installed"
    return
  fi

  sudo apt-get update --quiet
  sudo DEBIAN_FRONTEND=noninteractive apt-get install --yes --quiet --no-install-recommends \
    chromium-headless-shell
  chromium-headless-shell --version
}

install_cosign
install_headless_shell
