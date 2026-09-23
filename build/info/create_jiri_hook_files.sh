#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script is expected to be invoked as a Jiri hook from
# integration/fuchsia/stem.

# Its purpose is to generate a Unix timestamp and a commit hash
# file under //build/info/jiri_generated/
#
# For context, see https://fxbug.dev/335391299
#
# This uses the //build/info/gen_latest_commit_date.py script
# with the --force-git option to generate them. Without this
# option, the same script, which will be invoked at build time,
# will read these files as input instead, and process them before
# writing them to their final destination in the Ninja or Bazel
# build artifact directories.

_SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

fatal () {
  echo >&2 "FATAL: $*"
  exit 1
}

# Assume this script lives under //build/info/
FUCHSIA_DIR="$(cd "${_SCRIPT_DIR}/../.." && pwd -P 2>/dev/null)"
if [[ ! -f "${FUCHSIA_DIR}/.jiri_manifest" ]]; then
  fatal "Cannot locate proper FUCHSIA_DIR, got: ${FUCHSIA_DIR}"
fi

OUTPUT_DIR="${FUCHSIA_DIR}/build/info/jiri_generated"
if [[ ! -f "${OUTPUT_DIR}/README.md" ]]; then
  fatal "Cannot locate output directory (missing README.md): ${OUTPUT_DIR}"
fi

# Call git directly, as Python is not available when Jiri hooks run on infra bots.
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_OPTIONAL_LOCKS=0
export GIT_CONFIG_NOSYSTEM=1

INTEGRATION_DIR="${FUCHSIA_DIR}/integration"
INTEGRATION_HASH="$(git -C "$INTEGRATION_DIR" rev-parse HEAD)"
INTEGRATION_STAMP="$(git -C "$INTEGRATION_DIR" log -n1 --date=unix --format=%cd)"

# Get the same metadata (hash and commit timestamp) about the last commit to
# fuchsia.git on the day before the day that JIRI_HEAD was committed

# Get the date when JIRI_HEAD was committed, which is probably today. Looks like
# `YYYY-MM-DD`.
TODAY="$( TZ=UTC0 git log -n1 --format=%cd --date=format-local:%Y-%m-%d JIRI_HEAD)"
DAILY_HASH="$(git log -n1 --until "${TODAY}T00:00:00Z" --format=%H)"
DAILY_STAMP="$(git log -n1 --until "${TODAY}T00:00:00Z" --date=unix --format=%cd)"

# Write $1 to the path given in $2, but only if doing so would actually change
# $2.
function _write_if_changed() {
  local contents="$1"
  local path="$2"

  if [ ! -r "$path" ] || [ "$(<"$path")" != "$contents" ]; then
    echo "$contents" > "$path"
  fi
}

# LINT.IfChange
_write_if_changed "$INTEGRATION_HASH" "${OUTPUT_DIR}/integration_commit_hash.txt"
_write_if_changed "$INTEGRATION_STAMP" "${OUTPUT_DIR}/integration_commit_stamp.txt"
_write_if_changed "$DAILY_HASH" "${OUTPUT_DIR}/integration_daily_commit_hash.txt"
_write_if_changed "$DAILY_STAMP" "${OUTPUT_DIR}/integration_daily_commit_stamp.txt"
# LINT.ThenChange(//build/info/BUILD.gn)
