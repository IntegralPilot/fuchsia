#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -eu

SCRIPT_DIR=$(dirname "${BASH_SOURCE[0]}")
BAZEL_ENSURE_FILE="${SCRIPT_DIR}/cipd_manifests/bazel.ensure"
WORKSPACE_ROOT=""
CIPD_BIN=""
VERBOSE=0
CIPD_LOG_LEVEL="info"

source "$SCRIPT_DIR/common.sh" || exit $?

help() {
  echo
  echo "Script used to bootstrap the bazel binary"
  echo
  echo "Usage:"
  echo "   $(basename "$0") [<options>]"
  echo
  echo "Options:"
  echo
  echo "  -h"
  echo "     Prints this help message"
  echo "  -v"
  echo "     Uses verbose output"
  echo
}

function log {
  if [[ "$VERBOSE" -eq 1 ]]; then
    echo "$@"
  fi
}

function main() {
  while getopts ":hv" opt; do
    case ${opt} in
      'h' | '?')
        help
        exit 1
        ;;
      'v')
        VERBOSE=1
        CIPD_LOG_LEVEL="debug"
        ;;
    esac
  done

  WORKSPACE_ROOT=$(get_workspace_root)
  log "Found WORKSPACE file at ${WORKSPACE_ROOT}"

  if command -v cipd &> /dev/null; then
    CIPD_BIN="cipd"
    log "cipd client already on path, skipping download"
  else
    CIPD_BIN="${WORKSPACE_ROOT}/.cipd_client"
    "${SCRIPT_DIR}/bootstrap_cipd.sh" "${CIPD_BIN}"
    log "Downloaded cipd client to ${CIPD_BIN}"
  fi

  "${CIPD_BIN}" ensure -ensure-file "${BAZEL_ENSURE_FILE}" -root "${WORKSPACE_ROOT}" -log-level="${CIPD_LOG_LEVEL}"
  log
  log "Success: downloaded bazel to ${WORKSPACE_ROOT}/tools/bazel"
}

main "$@"



