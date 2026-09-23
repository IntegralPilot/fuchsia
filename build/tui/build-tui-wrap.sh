#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# build-tui-wrap.sh invokes rsclient's buildminder-wrap.sh,
# and automatically passes important options.

set -euo pipefail

readonly SCRIPT_NAME="$(basename "${BASH_SOURCE[0]}")"
readonly SCRIPT_DIR="$(dirname "${BASH_SOURCE[0]}")"

# Get the HOST_PLATFORM for the prebuilt path.
# Sourcing platform.sh requires FUCHSIA_DIR to be set.
readonly FUCHSIA_DIR="$(readlink -f "$SCRIPT_DIR/../..")"
source "${FUCHSIA_DIR}/tools/devshell/lib/platform.sh"

# rsclient install path is set in manifests/prebuilts
readonly PREBUILT_RSCLIENT_DIR="${FUCHSIA_DIR}/prebuilt/rsclient/$HOST_PLATFORM"
readonly buildminder_wrap="$PREBUILT_RSCLIENT_DIR/bin/buildminder-wrap.sh"
readonly buildminder="$PREBUILT_RSCLIENT_DIR/bin/buildminder"

# default options:
verbose=0

function die() {
  echo "[$SCRIPT_NAME]: Error: $*" >&2
  exit 1
}

function ts_echo() {
  if [[ "$verbose" -ne 0 ]]; then
    echo "[@$EPOCHREALTIME] [$SCRIPT_NAME]: $*" >&2
  fi
}

function usage() {
  cat <<EOF
usage: $0 [options] -- command ...

options:
  -h | --help: print help and exit
  --log-dir DIR: buildminder log dir
  -v | --verbose: print debug messages

  Unrecognized options before -- will be forwarded to buildminder.
EOF
}

# Parse options up to --, and treat the rest as the wrapped command.
buildminder_options=()
got_ddash=0
log_dir=
prev_opt=
for opt  # "$@"
do
  # handle --option arg
  if [[ -n "$prev_opt" ]]
  then
    eval "$prev_opt"=\$opt
    prev_opt=
    shift
    continue
  fi

  # Extract optarg from --opt=optarg
  optarg=
  case "$opt" in
    -*=*) optarg="${opt#*=}" ;;  # remove-prefix, shortest-match
  esac

  case "$opt" in
    -h | --help) usage; exit ;;
    --log-dir=*) log_dir="$optarg" ;;
    --log-dir) prev_opt=log_dir ;;
    -v | --verbose) verbose=1 ;;

    --) got_ddash=1; shift; break ;;

    # Forward unknown options to buildminder.
    *) buildminder_options+=( "$opt" ) ;;
  esac
  shift
done

[[ -z "$prev_opt" ]] || {
  die "Missing --${prev_opt} argument."
}

wrapped_command=("$@")

[[ "$got_ddash" == 1 ]] || {
  die "Missing -- before the wrapped command."
}
[[ "${#wrapped_command[@]}" -ge 1 ]] || {
  die "The wrapped command must not be empty."
}

buildminder_wrap_options=(
  --buildminder "$buildminder"
  --mode "integrated"
)

# Handle log dir.
if [[ -n "$log_dir" ]]
then
  buildminder_wrap_options+=( --log-dir "$log_dir" )
fi
# Otherwise, fallback to using some temp dir.

# Ensure that the prebuilt python3 is in the PATH (needed in infra environment).
# buildminder-wrap.sh uses python3 as an alternative means for sleep.
readonly py3_bindir="${PREBUILT_PYTHON3%/*}"  # dirname
export PATH="$py3_bindir:$PATH"

full_cmd=(
  "${buildminder_wrap}"
  "${buildminder_wrap_options[@]}"
  --buildminder_options
  "${buildminder_options[@]}"
  --
  "${wrapped_command[@]}"
)

[[ "$verbose" == 0 ]] || {
  echo "[$SCRIPT_NAME] ---- env start ----"
  # Uncomment the following to enable extreme gRPC traffic logging from buildminder.
  # export SH_WRAPPER_TEST_DEBUG=1
  env
  echo "[$SCRIPT_NAME] ---- env end ----"
}

ts_echo "full command: ${full_cmd[*]}"
exec "${full_cmd[@]}"
