#!/usr/bin/env bash
set -Eeuo pipefail

INSTALL_DIR=${PCTL_INSTALL_DIR:-/usr/local/bin}
BINARY_PATH="${INSTALL_DIR}/pctl"
REPOSITORY=bernard-ng/pctl

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

[[ $(uname -s) == Linux ]] || die "this installer supports Linux hosts and containers"

case $(uname -m) in
  x86_64 | amd64)
    architecture=x86_64
    ;;
  aarch64 | arm64)
    architecture=aarch64
    ;;
  *)
    die "unsupported architecture: $(uname -m)"
    ;;
esac

require_command curl
require_command install
require_command mktemp
require_command mv
require_command tar

archive_url=${PCTL_BINARY_URL:-https://github.com/${REPOSITORY}/releases/latest/download/pctl-linux-${architecture}.tar.gz}
download_dir=$(mktemp -d)
archive_path="${download_dir}/pctl.tar.gz"
candidate_binary="${download_dir}/pctl"
staged_binary="${BINARY_PATH}.new"

cleanup() {
  rm -rf -- "$download_dir"
  rm -f -- "$staged_binary"
}

trap cleanup EXIT

printf 'Downloading the latest pctl release for Linux %s...\n' "$architecture"
curl --fail --location --show-error --silent --retry 3 --output "$archive_path" "$archive_url"
tar -xOzf "$archive_path" pctl >"$candidate_binary" || die "release archive does not contain pctl"
chmod 0755 "$candidate_binary"
if ! installed_version=$("$candidate_binary" --version 2>&1); then
  die "pctl cannot run on this system: ${installed_version}"
fi

install -d -m 0755 "$INSTALL_DIR" || die "cannot create ${INSTALL_DIR}; run the installer with sufficient permissions"
install -m 0755 "$candidate_binary" "$staged_binary" || die "cannot write to ${INSTALL_DIR}; run the installer with sufficient permissions"
mv -f -- "$staged_binary" "$BINARY_PATH"

printf 'Installed %s to %s\n' "$installed_version" "$BINARY_PATH"
