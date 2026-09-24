#!/usr/bin/env bash
set -Eeuo pipefail

INSTALL_DIR=${PCTL_INSTALL_DIR:-/usr/local/bin}
BINARY_PATH="${INSTALL_DIR}/pctl"

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

confirm_removal() {
  local answer=

  printf 'This will remove %s. Project manifests and local state will be preserved.\n' "$BINARY_PATH" >&2
  read -r -p 'Continue? [y/N] ' answer </dev/tty || die "cannot read confirmation from /dev/tty (use --yes for unattended removal)"
  [[ $answer == [yY] || $answer == [yY][eE][sS] ]] || {
    printf 'Uninstall cancelled.\n'
    exit 0
  }
}

assume_yes=false
case ${1:-} in
  --yes | -y)
    assume_yes=true
    ;;
  '')
    ;;
  *)
    die "usage: $0 [--yes]"
    ;;
esac

[[ $# -le 1 ]] || die "usage: $0 [--yes]"

if [[ $assume_yes != true ]]; then
  confirm_removal
fi

if [[ ! -e $BINARY_PATH ]]; then
  printf 'pctl is not installed at %s.\n' "$BINARY_PATH"
  exit 0
fi

rm -f -- "$BINARY_PATH" || die "cannot remove ${BINARY_PATH}; run the uninstaller with sufficient permissions"
printf 'Removed %s.\n' "$BINARY_PATH"
