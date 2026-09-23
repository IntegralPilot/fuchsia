#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -eu

SCRIPT_DIR=$(dirname "${BASH_SOURCE[0]}")
VERSION_FILE="${SCRIPT_DIR}/../cipd/private/cipd_digests/cipd_client_version"
CIPD_BACKEND="https://chrome-infra-packages.appspot.com"

PLATFORM=""

function get_platform() {
  local arch=""
  local os=""

  local uname=$(uname -s | tr '[:upper:]' '[:lower:]')
  case "${uname}" in
    linux)
      os="${uname}"
      ;;
    darwin)
      os=mac
      ;;
    *)
      >&2 echo "Bazel not supported on ${uname}"
      exit 1
  esac

  if [ -z "${arch}" ]; then
    uname=$(uname -m | tr '[:upper:]' '[:lower:]')
    case "${uname}" in
      x86_64|amd64)
        arch=amd64
        ;;
      s390x|ppc64|ppc64le)  # best-effort support
        arch="${uname}"
        ;;
      aarch64)
        arch=arm64
        ;;
      armv7l)
        arch=armv6l
        ;;
      arm*)
        arch="${uname}"
        ;;
      *86)
        arch=386
        ;;
      *)
        >&2 echo "UNKNOWN Machine architecture: ${uname}"
        exit 1
    esac
  fi
  PLATFORM="${os}-${arch}"
}

# expected_sha256 reads the expected SHA256 hex digest from *.digests file.
#
# Args:
#   Name of the platform to get client's digest for.
# Stdout:
#   Lowercase SHA256 hex digest.
function expected_sha256() {
  local line
  while read -r line; do
    if [[ "${line}" =~ ^([0-9a-z\-]+)[[:blank:]]+sha256[[:blank:]]+([0-9a-f]+)$ ]] ; then
      local plat="${BASH_REMATCH[1]}"
      local hash="${BASH_REMATCH[2]}"
      if [ "${plat}" ==  "$1" ]; then
        echo "${hash}"
        return 0
      fi
    fi
  done < "${VERSION_FILE}.digests"

  >&2 echo -n "[31;1m"
  >&2 echo -n "Platform $1 is not supported by the CIPD client bootstrap: "
  >&2 echo -n "there's no pinned SHA256 hash for it in the *.digests file."
  >&2 echo "[0m"

  return 1
}

# calc_sha256 is "portable" variant of sha256sum. It uses sha256sum when
# available (most Linuxes and cygwin) and 'shasum -a 256' otherwise (for OSX).
#
# Args:
#   Path to a file.
# Stdout:
#   Lowercase SHA256 hex digest of the file.
function calc_sha256() {
  if hash sha256sum 2> /dev/null ; then
    sha256sum "$1" | cut -d' ' -f1
  elif hash shasum 2> /dev/null ; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    >&2 echo -n "[31;1m"
    >&2 echo -n "Don't know how to calculate SHA256 on your platform. "
    >&2 echo -n "Please use your package manager to install one before continuing:"
    >&2 echo
    >&2 echo "  sha256sum"
    >&2 echo -n "  shasum"
    >&2 echo "[0m"
    return 1
  fi
}

function download_cipd_client() {
  echo "Downloading cipd client to ${CLIENT}"

  local expected_hash=$(expected_sha256 "${PLATFORM}")
  if [ -z "${expected_hash}" ] ; then
    exit 1
  fi

  local client_version=$(cat "${VERSION_FILE}")
  local url="${CIPD_BACKEND}/client?platform=${PLATFORM}&version=${client_version}"

  local cipd_client_tmp=$(\
    mktemp -p "${SCRIPT_DIR}" 2>/dev/null || \
    mktemp "${SCRIPT_DIR}/.cipd_client.XXXXXXX")

  if hash curl 2> /dev/null ; then
    curl "${url}" -s --show-error -f --retry 3 --retry-delay 5 -L -o "${cipd_client_tmp}"
  elif hash wget 2> /dev/null ; then
    wget "${url}" -q -t 3 -w 5 --retry-connrefused -O "${cipd_client_tmp}"
  else
    >&2 echo -n "[31;1m"
    >&2 echo -n "Your platform is missing a supported fetch command. "
    >&2 echo "Please use your package manager to install one before continuing:"
    >&2 echo
    >&2 echo "  curl"
    >&2 echo "  wget"
    >&2 echo
    >&2 echo "Alternately, manually download:"
    >&2 echo "  ${url}"
    >&2 echo -n "To ${CLIENT}, and then re-run this command."
    >&2 echo "[0m"
    rm "${cipd_client_tmp}"
    exit 1
  fi

  local actual_hash=$(calc_sha256 "${cipd_client_tmp}")
  if [ -z "${actual_hash}" ] ; then
    rm "${cipd_client_tmp}"
    exit 1
  fi

  if [ "${actual_hash}" != "${expected_hash}" ]; then
    >&2 echo -n "[31;1m"
    >&2 echo "SHA256 digest of the downloaded CIPD client is incorrect:"
    >&2 echo "  Expecting ${expected_hash}"
    >&2 echo "  Got       ${actual_hash}"
    >&2 echo -n "Refusing to run it. Check that *.digests file is up-to-date."
    >&2 echo "[0m"
    rm "${cipd_client_tmp}"
    exit 1
  fi

  set +e
  chmod +x "${cipd_client_tmp}"
  mv "${cipd_client_tmp}" "${CLIENT}"
  set -e

}

function main() {
  if [ $# == 0 ] ; then
    >&2 echo -n "[31;1m"
    >&2 echo "${BASH_SOURCE[0]} requires a path to download the client to:"
    >&2 echo "[0m"
    exit 1
  fi
  CLIENT="${1}"
  get_platform
  download_cipd_client
}

main "$@"
