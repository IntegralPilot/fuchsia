#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This is a wrapper script that orchestrates multiple feature-enabling
# wrappers for running builds.
# Features:
#   --rbe : enable RBE using reproxy
#   --resultstore : enable uploading build metadata to ResultStore service
#   --profile : enable system profiling during build
#   --tui : enable terminal UI for monitoring build

set -euo pipefail

readonly SCRIPT_NAME="${BASH_SOURCE[0]##*/}"  # basename
readonly SCRIPT_DIR="${BASH_SOURCE[0]%/*}"  # dirname

# TODO: remove dependence on FUCHSIA_DIR and fuchsia-specific conventions,
# apart from locations of essential scripts and tools.
# Sourcing platform.sh requires FUCHSIA_DIR to be set.
readonly FUCHSIA_DIR="$(readlink -f "$SCRIPT_DIR/../..")"
source "${FUCHSIA_DIR}/tools/devshell/lib/platform.sh"

readonly profile_wrapper="${FUCHSIA_DIR}/build/profile/profile_wrap.sh"
readonly hardware_profiler="${FUCHSIA_DIR}/build/profile/hardware_profiler.py"
readonly reproxy_wrapper="${FUCHSIA_DIR}/build/rbe/fuchsia-reproxy-wrap.sh"
readonly rsproxy_wrapper="${FUCHSIA_DIR}/build/resultstore/fuchsia-rsproxy-wrap.sh"
readonly tui_wrapper="${FUCHSIA_DIR}/build/tui/build-tui-wrap.sh"

verbose=0
function debug() {
  [[ "$verbose" -eq 0 ]] || echo "[$SCRIPT_NAME]: $*"
}

function ts_echo() {
  if [[ "$verbose" -ne 0 ]]; then
    # Use high-precision epoch timestamps for easier debugging and performance profiling.
    echo "[@$EPOCHREALTIME] [$SCRIPT_NAME]: $*" >&2
  fi
}

function die() {
  echo "[$SCRIPT_NAME] Error: $*"
  exit 1
}

function usage() {
  cat <<EOF
usage: $SCRIPT_NAME [options] -- build-command...

options:
  -h | --help : print help and exit
  -v | --verbose : run verbosely
  --dry-run : print the final command instead of running it

  --build-dir DIR : (required) build output dir (depth=2, e.g. "out/foo")
      When building with ninja, the -C argument is interpreted as the build dir.
  --log-dir DIR : where to produce logs for various build tools.
      Defaults to using a timestamp-based directory name under out/_build_logs.

  --loas-type : LOAS type used to choose authentication method
      values: restricted, unrestricted, auto, skip

  --use-machine-credentials : use GCE machine-credentials (bypasses LOAS/OAuth checks, takes absolute precedence)

  --profile[=0] : enable system profiling during build

  --rbe[=0] : enable support for remote execution
  --reproxy-cfg : additional reproxy configs to merge and use

  --resultstore[=0] : uploading build metadata and results to ResultStore
  --pre-build-uploads : configure-time invocation artifacts to upload
  --post-build-uploads : end-of-build invocation artifacts to upload

  --tui : enable terminal UI for monitoring build

  The wrapped build command follows --.
EOF
}

### Defaults.
collect_system_profile=0  # fx build-profile
dry_run=0
enable_resultstore=0
needs_reproxy_rbe=0
tui=0
use_gce_machine_credentials=0

### Configuration.
# Parse command-line arguments.
build_dir=
log_dir=
loas_type=
reproxy_cfgs=()
pre_build_uploads=()
post_build_uploads=()
build_tool_args=()  # The wrapped command
prev_opt=
prev_opt_append=
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
  if [[ -n "$prev_opt_append" ]]
  then
    eval "$prev_opt_append"+=\(\$opt\)
    prev_opt_append=
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
    -v | --verbose) verbose=1 ;;
    --dry-run) dry_run=1 ;;

    --build-dir=*) build_dir="$optarg" ;;
    --build-dir) prev_opt=build_dir ;;

    --log-dir=*) log_dir="$optarg" ;;
    --log-dir) prev_opt=log_dir ;;

    --loas-type=*) loas_type="$optarg" ;;
    --loas-type) prev_opt=loas_type ;;

    --use-machine-credentials) use_gce_machine_credentials=1 ;;

    --profile=*) collect_system_profile="$optarg" ;;
    --profile) collect_system_profile=1 ;;

    --rbe=*) needs_reproxy_rbe="$optarg" ;;
    --rbe) needs_reproxy_rbe=1 ;;
    --reproxy-cfg=*) reproxy_cfgs+=( "$optarg" ) ;;
    --reproxy-cfg) prev_opt_append=reproxy_cfgs ;;

    --resultstore=*) enable_resultstore="$optarg" ;;
    --resultstore) enable_resultstore=1 ;;
    --pre-build-uploads=*) pre_build_uploads+=( "$optarg" ) ;;
    --pre-build-uploads) prev_opt_append=pre_build_uploads ;;
    --post-build-uploads=*) post_build_uploads+=( "$optarg" ) ;;
    --post-build-uploads) prev_opt_append=post_build_uploads ;;

    --tui) tui=1 ;;

    # Stop option processing.
    --) shift; break ;;
  esac
  shift
done

[[ -z "$prev_opt" ]] || {
  die "Missing --${prev_opt} argument."
}
[[ -z "$prev_opt_append" ]] || {
  die "Missing --${prev_opt_append} argument."
}

build_tool_args+=( "$@" )
[[ "${#build_tool_args[@]}" -ge 1 ]] || {
  die "Missing wrapped command after --."
}

ts_echo "Starting build wrapper orchestration"

[[ -n "$build_dir" ]] || die "Missing required --build-dir option."

readonly build_dir_basename="${build_dir##*/}"  # basename

[[ -n "$log_dir" ]] || {
  # Then choose our own log dir, based on $build_dir.
  readonly out_dir_root="${FUCHSIA_DIR}/out"
  readonly build_logs_root="$out_dir_root/_build_logs"
  readonly log_dir_base="$build_logs_root/$build_dir_basename"
  mkdir -p "$log_dir_base"
  readonly timestamp="$(date +%Y%m%d-%H%M%S)"
  log_dir="$(mktemp -d "${log_dir_base}/build.${timestamp}.XXXXXXXX")"
  debug "Using newly created log dir: $log_dir"
}

loas_type_arg=()
[[ -z "$loas_type" ]] || loas_type_arg+=( --loas-type "$loas_type" )

### Composition.
# Stack prefix wrappers based on enabled features.

maybe_tui_wrap=()
if [[ "$tui" -eq 1 ]]
then
  debug "TUI enabled."
  readonly tui_logdir="$log_dir/tui_logs"
  maybe_tui_wrap=(
    "$tui_wrapper"
    --log-dir "$tui_logdir"
  )
  [[ "$verbose" -eq 0 ]] || maybe_tui_wrap+=( --verbose )
  maybe_tui_wrap+=( -- )
  # Nothing to upload.  But if we ever want to upload artifacts such as logs
  # or checkpoints to ResultStore, this wrapper layer must move inside the
  # resultstore (rsproxy) wrapper layer below.

  # If fint ever wants any artifacts from the TUI, the their wrap positions
  # need to be swapped.
fi

maybe_fint_build_wrap=()
# TODO: support fint build wrapper (for infra)

maybe_profile_wrap=()
if [[ "$collect_system_profile" -eq 1 ]]
then
  debug "Profiling enabled."
  # LINT.IfChange(build_profile_filenames)
  readonly profile_log_dir="$log_dir/build_profile"
  mkdir -p "$profile_log_dir"
  readonly system_profile_log="${profile_log_dir}/system_profile.json"
  readonly hardware_profile_log="${profile_log_dir}/hardware_profile.json"
  # LINT.ThenChange(//build/scripts/main_build.py:build_profile_filenames)

  # Determine workspace cache file path (under the parsed build_dir)
  readonly cache_file="${build_dir}/hardware_profile.cache.json"

  if [[ ! -f "$cache_file" ]]; then
    debug "Host hardware profile cache missing. Scanning host topology..."
    "${PREBUILT_PYTHON3:-python3}" -S "${hardware_profiler}" --output "$cache_file"
  fi

  # Copy the cached profile to the current build's profile folder synchronously in <1ms.
  if [[ -f "$cache_file" ]]; then
    cp "$cache_file" "$hardware_profile_log"
  fi

  maybe_profile_wrap=(
    "$profile_wrapper"
    --system-log "$system_profile_log"
  )
  [[ "$verbose" -eq 0 ]] || maybe_profile_wrap+=( --verbose )
  maybe_profile_wrap+=( -- )
  post_build_uploads+=(
    # trace and capability logs
    "$system_profile_log"
    "$hardware_profile_log"
  )
fi

maybe_rbe_wrap=()
if [[ "$needs_reproxy_rbe" -eq 1 ]]
then
  debug "RBE enabled."
  readonly reproxy_logdir="$log_dir/reproxy_logs"
  mkdir -p "$reproxy_logdir"
  # reproxy works best when it uses a temp dir on the same physical device
  # as the build dir.
  # Reuse the timestamp-based part of $log_dir.
  readonly reproxy_tmpdir="$build_dir/.reproxy_tmpdirs/${log_dir##*/}"
  mkdir -p "$reproxy_tmpdir"

  reproxy_cfg_args=()
  for f in "${reproxy_cfgs[@]}"
  do reproxy_cfg_args+=( --cfg "$f" )
  done

  reproxy_shutdown_opts=()
  if [[ "$enable_resultstore" -eq 0 ]]
  then
    # When resultstore is enabled, we need to wait for reproxy to fully
    # shutdown to guarantee that produces the logs and metrics that will
    # be uploaded as post-build artifacts by rsproxy.
    # Otherwise, allow reproxy to shutdown asynchronously.
    reproxy_shutdown_opts+=( --async_reproxy_termination )
  fi

  maybe_rbe_wrap=(
    "$reproxy_wrapper"
    --logdir "$reproxy_logdir"
    --tmpdir "$reproxy_tmpdir"
    "${loas_type_arg[@]}"
  )
  [[ "$use_gce_machine_credentials" -eq 0 ]] || maybe_rbe_wrap+=( --use-machine-credentials )
  [[ "$verbose" -eq 0 ]] || maybe_rbe_wrap+=( --verbose )
  maybe_rbe_wrap+=(
    "${reproxy_cfg_args[@]}"
    "${reproxy_shutdown_opts[@]}"
    --
  )
  post_build_uploads+=(
    "$reproxy_logdir/rbe_metrics.txt"
    # TODO: consider compressing the reproxy log
    "$reproxy_logdir/reproxy.rrpl"
  )
fi

# Both ResultStore and the TUI use rsproxy.
maybe_resultstore_wrap=()
if [[ "$enable_resultstore" -eq 1 || "$tui" -eq 1 ]]
then
  if [[ "$enable_resultstore" -eq 1 ]]
  then debug "ResultStore uploads enabled, using rsproxy."
  fi
  if [[ "$tui" -eq 1 ]]
  then debug "TUI enabled, using rsproxy."
  fi
  readonly rsproxy_logdir="$log_dir/rsproxy_logs"

  rsproxy_options=()

  rsproxy_loas_type_arg=( "${loas_type_arg[@]}" )
  if [[ "$enable_resultstore" -eq 1 ]]
  then
    for f in "${pre_build_uploads[@]}"
    do rsproxy_options+=( --pre_build_uploads "$f" )
    done

    for f in "${post_build_uploads[@]}"
    do rsproxy_options+=( --post_build_uploads "$f" )
    done
  else
    # Use skip authentication and skip fuchsia-specific config
    # when only enabling for the TUI.
    rsproxy_loas_type_arg=( --loas-type skip )
  fi

  maybe_resultstore_wrap=(
    "$rsproxy_wrapper"
    "${rsproxy_loas_type_arg[@]}"
  )
  [[ "$use_gce_machine_credentials" -eq 0 ]] || maybe_resultstore_wrap+=( --use-machine-credentials )
  [[ "$verbose" -eq 0 ]] || maybe_resultstore_wrap+=( --verbose )
  maybe_resultstore_wrap+=(
    --log-dir "$rsproxy_logdir"
    "${rsproxy_options[@]}"
    --
  )
fi

### Execution.
readonly full_cmd=(
  "${maybe_tui_wrap[@]}"
  "${maybe_fint_build_wrap[@]}"
  "${maybe_resultstore_wrap[@]}"
  "${maybe_profile_wrap[@]}"
  "${maybe_rbe_wrap[@]}"

  "${build_tool_args[@]}"
)

debug "full command: ${full_cmd[*]}"
if [[ "$dry_run" -eq 1 ]]; then
  echo "${full_cmd[@]}"
else
  exec "${full_cmd[@]}"
fi

