#!/usr/bin/env bash
# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e -o pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

usage() {
    echo "Update lockfiles for CIPD packages."
    echo "    usage: update-lockfiles.sh [--jiri PATH_TO_JIRI]"
    exit 1
}

if [[ ($1 == "-h") || ($1 == "--help") ]]
then
  usage;
fi

# Looking for path to jiri
if [[ $1 == "--jiri" ]]
then
  if [[ "$#" != 2 ]]
  then
    usage
  fi
  JIRI_PATH=$2
else
  if ! which jiri > /dev/null
  then
    echo "jiri is not found in current \$PATH."
    echo "Please use --jiri PATH_TO_JIRI to specify the location of jiri."
    exit 2
  fi
  JIRI_PATH=`which jiri`
fi

update_lockfile() {
  local lockfile="$1"
  local manifest="$2"
  local logfile="${SCRIPT_DIR}/debug.log"
  # -local-manifest-project=fuchsia makes Jiri respect unmerged changes to
  # manifests in fuchsia.git.
  if ! ${JIRI_PATH} -v resolve -local-manifest-project=fuchsia \
    -output "$lockfile" "$manifest" > "$logfile" 2>&1
  then
    echo "Failed to update lockfile $1, jiri logs:"
    echo
    cat "$logfile"
    rm -f "$logfile"
    return 1
  fi
  rm -f "$logfile"
  return 0
}

update_lockfile \
  "${SCRIPT_DIR}/jiri.lock" \
  "${SCRIPT_DIR}/platform"

echo "lockfiles updated."
exit 0
