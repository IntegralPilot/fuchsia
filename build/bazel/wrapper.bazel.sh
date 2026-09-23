#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# A wrapper script used to invoke Bazel in the Fuchsia build's workspace.
# This requires a file named "bazel.sh.config" to be located in the same
# directory containing specific variable definitions (see below).
#
# Do not use directly, the //build/regenerator script will copy this
# file to a specific location in the build directory and generate the
# appropriate configuration file.
#
# Environment variables that influence this script:
#
# FX_INTERNAL_RESULTSTORE_BAZEL: Set to "resultstore" or "resultstore_infra"
#   to dynamically append the corresponding --config= flag at runtime.
#   Set to "0" or leave unset to disable.

_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"
readonly _SCRIPT_DIR

_SRCDIR="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" >/dev/null 2>&1 && pwd)"
readonly _SRCDIR

function die {
  echo >&2 "ERROR: $*"
  exit 1
}

function msg {
  # identify using basename of this script
  echo >&2 "[${BASH_SOURCE[0]##*/}] $*"
}

# Read the configuration file. This should define the following variables:
#
# _BAZEL_BIN: Path to Bazel launcher script.
# _BAZEL_OUTPUT_BASE: Bazel output base directory path.
# _BAZEL_OUTPUT_USER_ROOT: Bazel output user root directory path.
# _BAZEL_WORKSPACE: Bazel workspace directory path.
# _NINJA_BUILD_DIR: Ninja build directory path.
# _PREBUILT_NINJA: Prebuilt Ninja binary path.
# _PREBUILT_PYTHON_DIR: Prebuilt python binary path.
#
# All paths should be absolute.
#
# LINT.IfChange(bazel.sh.config)
readonly _SCRIPT_ARGS_FILE="${_SCRIPT_DIR}/bazel.sh.config"
# LINT.ThenChange(//build/bazel/scripts/workspace_utils.py:bazel.sh.config)
[[ -f "${_SCRIPT_ARGS_FILE}" ]] || die "Missing Bazel script config file: ${_SCRIPT_ARGS_FILE}"

# shellcheck source=/dev/null
source "${_SCRIPT_ARGS_FILE}"

[[ -n "${_BAZEL_WORKSPACE}" ]] || die "Missing _BAZEL_WORKSPACE config variable"
[[ -d "${_BAZEL_WORKSPACE}" ]] || die "_BAZEL_WORKSPACE should be a directory: ${_BAZEL_WORKSPACE}"

# These directories are created on demand.
[[ -n "${_BAZEL_OUTPUT_BASE}" ]] || die "Missing _BAZEL_OUTPUT_BASE config variable"
[[ -n "${_BAZEL_OUTPUT_USER_ROOT}" ]] || die "Missing _BAZEL_OUTPUT_USER_ROOT config variable"

[[ -n "${_BAZEL_BIN}" ]] || die "Missing _BAZEL_BIN config variable"
[[ -f "${_BAZEL_BIN}" ]] || die "_BAZEL_BIN should be a file: ${_BAZEL_BIN}"

[[ -n "${_NINJA_BUILD_DIR}" ]] || die "Missing _NINJA_BUILD_DIR config variable"
[[ -d "${_NINJA_BUILD_DIR}" ]] || die "_NINJA_BUILD_DIR should be a directory: ${_NINJA_BUILD_DIR}"

[[ -n "${_PREBUILT_NINJA}" ]] || die "Missing _PREBUILT_NINJA config variable"
[[ -f "${_PREBUILT_NINJA}" ]] || die "_PREBUILT_NINJA should be a file: ${_PREBUILT_NINJA}"

[[ -n "${_PREBUILT_PYTHON_DIR}" ]] || die "Missing _PREBUILT_PYTHON_DIR config variable"
[[ -d "${_PREBUILT_PYTHON_DIR}" ]] || die "_PREBUILT_PYTHON_DIR should be a directory: ${_PREBUILT_PYTHON_DIR}"

readonly _REMOTE_SERVICES_BAZELRC="${_NINJA_BUILD_DIR}/regenerator_outputs/remote_services.bazelrc"

readonly _GENERATE_INVOCATION_BAZELRC="${_SRCDIR}/scripts/generate_invocation_bazelrc.py"
readonly _INVOCATION_BAZELRC="${_BAZEL_WORKSPACE}/invocation.bazelrc"
readonly _VENDORING_BAZELRC="${_SRCDIR}/enable_vendor_mode.bazelrc"

# Exported explicitly to be used by repository rules to reference the
# Ninja output directory and binary.
export BAZEL_FUCHSIA_NINJA_OUTPUT_DIR="${_NINJA_BUILD_DIR}"
export BAZEL_FUCHSIA_NINJA_PREBUILT="${_NINJA_PREBUILT}"

# Ensure our prebuilt Python3 executable is in the PATH to run repository
# rules that invoke Python programs correctly in containers or jails that
# do not expose the system-installed one.
export PATH="${_PREBUILT_PYTHON_DIR}/bin:${PATH}"

# Provide a direct path to the prebuilt Python3 executable as well.
readonly PREBUILT_PYTHON3="${_PREBUILT_PYTHON_DIR}/bin/python3"

# An undocumented, but widely used, environment variable that tells Bazel to
# not auto-detect the host C++ installation. This makes workspace setup faster
# and ensures this can be used on containers where GCC or Clang are not
# installed (Bazel would complain otherwise with an error).
export BAZEL_DO_NOT_DETECT_CPP_TOOLCHAIN=1

# Determines the command used in this invocation, and separate arguments
# that follow a -- from the rest, so we can inject extra arguments
# *before* it, as this is crucial to properly wrap invocations such
# as `bazel run <config_args> -- <target> <cmd_args>`.
#
# This sets the following global variable:
#
# _BAZEL_COMMAND: The bazel command (e.g. "version", "info" or "build")
# _BAZEL_PRE_COMMAND_ARGS: All options that appear before the command
# _BAZEL_POST_COMMAND_ARGS: All options that appear after the command,
#     up to -- if provided.
# _BAZEL_REST_ARGS: Empty list, or "--" followed by other arguments that
#     follow it.
#
function parse_bazel_command () {
  _BAZEL_COMMAND=
  _BAZEL_PRE_COMMAND_ARGS=()
  _BAZEL_POST_COMMAND_ARGS=()
  _BAZEL_REST_ARGS=()

  while [[ "${#@}" -gt 0 ]]; do
    case "$1" in
      --)
       _BAZEL_REST_ARGS=("$@")
       break
       ;;
      -*)
       if [[ -z "${_BAZEL_COMMAND}" ]]; then
         _BAZEL_PRE_COMMAND_ARGS+=("$1")
       else
         _BAZEL_POST_COMMAND_ARGS+=("$1")
       fi
       ;;
      *)
       if [[ -z "${_BAZEL_COMMAND}" ]]; then
         _BAZEL_COMMAND="$1"
       else
         _BAZEL_POST_COMMAND_ARGS+=("$1")
       fi
       ;;
    esac
    shift
  done
}

parse_bazel_command "$@"

# The original invocation without -- and the args that follow it.
_BAZEL_DIRECT_ARGS=(
    "${_BAZEL_PRE_COMMAND_ARGS[@]}"
    "${_BAZEL_COMMAND}"
    "${_BAZEL_POST_COMMAND_ARGS[@]}"
)

# Make bazel_command_does_configuration non-empty when the current
# Bazel command requires analysis / build configurations.
bazel_command_does_configuration=
case "${_BAZEL_COMMAND}" in
  cquery | aquery | build | run | test | vendor)
      bazel_command_does_configuration=true
      ;;
esac

# We generate our own invocation id, to make it easier to propagate its value
# to sub-build invocations.
readonly RESULTSTORE_invocation_id="$("${PREBUILT_PYTHON3}" -S -c 'import uuid; print(uuid.uuid4())')"

# This is for ResultStore.
# TODO: depend on GN arg bazel_upload_build_events = {"sponge", "resultstore"}
readonly RESULTSTORE_URL="http://go/fxbtx"
readonly RESULTSTORE_SUB_BUILDS_LINK="$RESULTSTORE_URL/?q=PARENT_BUILD_ID:$RESULTSTORE_invocation_id"

# A list of extra Bazel arguments that must appear after the
# command.
_BAZEL_EXTRA_ARGS=(
  --invocation_id="$RESULTSTORE_invocation_id"
)

# LINT.IfChange(resultstore_bazel_env_vars)
# Enable ResultStore reporting if requested by the orchestrator.
# FX_INTERNAL_RESULTSTORE_BAZEL (set by build/scripts/main_build.py)
# can be set to "resultstore" or "resultstore_infra"
# (configs defined in build/bazel/templates/template.remote_services.gni).
case "${FX_INTERNAL_RESULTSTORE_BAZEL:-0}" in
  "" | "0")
    # Disabled or empty, do nothing
    ;;
  resultstore | resultstore_infra)
    # The resultstore configs (build:resultstore, build:resultstore_infra) are only
    # defined for commands that use build configuration. Subcommands like query do not
    # recognize build: configs.
    if [[ -n "${bazel_command_does_configuration}" ]]; then
      _BAZEL_EXTRA_ARGS+=( --config="${FX_INTERNAL_RESULTSTORE_BAZEL}" )
    fi
    ;;
  *)
    die "Invalid FX_INTERNAL_RESULTSTORE_BAZEL value: ${FX_INTERNAL_RESULTSTORE_BAZEL:-}. Expected 'resultstore' or 'resultstore_infra'."
    ;;
esac
# LINT.ThenChange(//build/scripts/main_build.py:resultstore_bazel_env_vars)

# The following step is sensitive to special environment variables:
#   * per-invocation build metadata
#       When uploading results to build event services like ResultStore or
#       Sponge, include extra metadata that links related invocations.
#   * proxy overrides sockets for remote services
#       For infra builds, connections to various remote services are tunneled
#       through local socket relays, launched by
#       [infra/infra]/cmd/buildproxywrap/main.go.
# These environment-sensitive modifications to config definitions
# manifest in the short-lived 'invocation.bazelrc'.
# Delete after use.
trap "rm -f ${_INVOCATION_BAZELRC}" EXIT
"${_GENERATE_INVOCATION_BAZELRC}" \
  --sub_builds_link="$RESULTSTORE_SUB_BUILDS_LINK" \
  > "${_INVOCATION_BAZELRC}"

# Save per-invocation logs.
# Log directories are generated dynamically per fx-build invocation,
# organized by bazel invocations.
readonly _BAZEL_INVOCATION_DATE="$(date +%Y%m%d-%H%M%S)"
readonly _BAZEL_INVOCATION_LOG_DIR_NAME="invocation-${_BAZEL_INVOCATION_DATE}--${RESULTSTORE_invocation_id}"

if [[ -n "$FX_BUILD_LOGDIR" ]]; then
  readonly _BAZEL_INVOCATION_LOG_PARENT="$FX_BUILD_LOGDIR/bazel_logs"
else
  # This can happen if this script is run outside of `fx build`.
  readonly _BAZEL_INVOCATION_LOG_PARENT="${_NINJA_BUILD_DIR}/bazel_logs"
  echo >&2 "WARNING: FX_BUILD_LOGDIR is not set. Using fallback log directory: ${_BAZEL_INVOCATION_LOG_PARENT}"
fi
readonly _BAZEL_INVOCATION_LOG_DIR="${_BAZEL_INVOCATION_LOG_PARENT}/${_BAZEL_INVOCATION_LOG_DIR_NAME}"
mkdir -p "${_BAZEL_INVOCATION_LOG_DIR}"

# Override log locations for this specific invocation.
# Bazel doesn't have a single log-dir control for writing all logs,
# so we must direct each one individually.
cat >> "${_INVOCATION_BAZELRC}" <<EOF
# Logs for this specific invocation.
common --experimental_workspace_rules_log_file=${_BAZEL_INVOCATION_LOG_DIR}/workspace_events.log
common:exec_log --execution_log_compact_file=${_BAZEL_INVOCATION_LOG_DIR}/exec_log.pb.zstd
common:_profiling --profile=${_BAZEL_INVOCATION_LOG_DIR}/command.profile.gz
EOF

# Save a copy of the final invocation.bazelrc for debugging.
cp "${_INVOCATION_BAZELRC}" "${_BAZEL_INVOCATION_LOG_DIR}/invocation.bazelrc"

# Maintain a 'recent' symlink to the latest log directory.
# Use a relative path for the symlink target so it works in archives.
ln -snf "${_BAZEL_INVOCATION_LOG_DIR_NAME}" "${_BAZEL_INVOCATION_LOG_PARENT}/recent"

# For convenience, link from the top_dir to the most recent "bazel_logs" dir.
# Note that _BAZEL_WORKSPACE is top_dir/workspace.
ln -snf "${_BAZEL_INVOCATION_LOG_PARENT}" "${_BAZEL_WORKSPACE}/../bazel_logs"


# Non-remote configuration permits use of a disk-cache, below.
has_remote_config=
for arg in "${_BAZEL_DIRECT_ARGS[@]}"
do
  # Check for infra and non-infra config variations to allow for local testing.
  # "sponge" and "resultstore" come from 'build/bazel/remote_services.gni',
  # and are added in 'build/bazel/bazel_action.gni'.
  # TODO(https://fxbug.dev/445093719): This method doesn't work if the same
  # configs are indirectly enabled.
  case "$arg" in
    --config=remote | --config=remote_cache_only)  # Remote build execution service
      has_remote_config=true
      ;;
  esac
done

# In Corp environments with valid gcert credentials, use the credential helper
# to automatically exchange LOAS for OAuth (Google Cloud Platform) tokens.
# This requires less interaction from the user than 'gcloud auth ...'.
#
# Note: --config=gcertauth is only appended when FX_BUILD_LOAS_TYPE is
# "unrestricted". When running on GCE bots or other machine-based environments
# where FX_BUILD_LOAS_TYPE is "skip" or absent, --config=gcertauth is omitted,
# and Bazel automatically falls back to using GCE VM Metadata Server credentials
# (via --google_default_credentials=true defined in remote_services.bazelrc).
use_gcert_auth=()
[[ "$FX_BUILD_LOAS_TYPE" == "unrestricted" ]] && {
  use_gcert_auth=(--config=gcertauth)
  # Don't set this, otherwise bazel will look for it
  # (and fail if it doesn't exist).
  unset GOOGLE_APPLICATION_CREDENTIALS
}

_BAZEL_PRE_COMMAND_ARGS+=(

  # Set a max idle duration of 10 minutes (default is 3 hours)
  # To avoid Ninja build timeouts on CI. See https://fxbug.dev/498320348
  --max_idle_secs=600

  # Do not parse $HOME/.bazelrc
  --nohome_rc

  # --nosystem_rc prevent parsing /etc/bazel.bazelrc, but prints a WARNING
  # to stderr on each invocation which is annoyingly noisy. Uncomment the
  # line below once this is fixed. See https://fxbug.dev/445090005.
  # --nosystem_rc

  # Use the output base and user root specific to the current
  # Fuchsia build directory, instead of default locations that
  # are under $HOME/.bazel/
  --output_base="${_BAZEL_OUTPUT_BASE}"
  --output_user_root="${_BAZEL_OUTPUT_USER_ROOT}"

  # Parse the .bazelrc defining configs related to remote builds.
  # TODO(digit): Import this from the workspace's .bazelrc file.
  --bazelrc="${_REMOTE_SERVICES_BAZELRC}"

  # For ephemeral configuration that amends remote_services.bazelrc:
  --bazelrc="${_INVOCATION_BAZELRC}"
)

# Except when running the 'vendor' command itself, include
# .bazelrc arguments to enable the use of our custom
# Bazel registry and vendor directory.
if [[ "${_BAZEL_COMMAND}" != "vendor" ]]; then
  _BAZEL_PRE_COMMAND_ARGS+=(
    --bazelrc="${_VENDORING_BAZELRC}"
  )
fi

# Use a shared disk cache if FUCHSIA_BAZEL_DISK_CACHE is set. Bazel
# documentation states that --disk_cache is compatible with remote caching.
#
# This is useful when several checkouts are used on the same machine,
# or when performing repeated clean builds frequently.
#
# Define FUCHSIA_BAZEL_DISK_CACHE_SIZE to enable automatic garbage
# collection, which will be performed by Bazel in the background when idle.
# If undefined, the cache is unlimited, and users will need to clean it
# manually.
[[ -n "${FUCHSIA_BAZEL_DISK_CACHE}" && -n "${_BAZEL_COMMAND}" ]] && {
  if [[ "${FUCHSIA_BAZEL_DISK_CACHE}" =~ ^/ ]]; then
    _BAZEL_EXTRA_ARGS+=(--disk_cache="${FUCHSIA_BAZEL_DISK_CACHE}")
    if [[ -n "${FUCHSIA_BAZEL_DISK_CACHE_SIZE}" ]]; then
      # The size should be a size in bytes, optionally followed by K, M, G or T.
      _BAZEL_EXTRA_ARGS+=(--experimental_disk_cache_gc_max_size="${FUCHSIA_BAZEL_DISK_CACHE_SIZE}")
    fi
  else
    echo >&2 "ERROR: FUCHSIA_BAZEL_DISK_CACHE ignored (not absolute): $FUCHSIA_BAZEL_DISK_CACHE"
    exit 1
  fi
}

# Add remote related arguments for configuration-sensitive commands.
# It is likely that this is only required for commands that build artifacts
# (i.e. build, run, test but not cquery and aquery) but err on the side of
# caution.
[[ -n "${bazel_command_does_configuration}" ]] &&
  _BAZEL_EXTRA_ARGS+=(
    "${use_gcert_auth[@]}"
    --config=_profiling
  )

# Setting $USER so `bazel` won't fail in environments with fake UIDs. Even if
# the USER is not actually used. See https://fxbug.dev/42063551#c9.
# In developer environments, use the real username so that authentication
# and credential helpers will work, e.g. go/bazel-google-sso.
_user="${USER:-unused-bazel-build-user}"

_bazel_env=(
    env USER="${_user}"

    # Inform sub-builds of this bazel invocation where to find related builds.
    # These environment variables are read by generate_invocation_bazelrc.py
    # and rsninja.sh, but do not impact actions from this bazel invocation.
    # LINT.IfChange(related_invocations_env_vars)
    RESULTSTORE_PARENT_BUILD_ID="$RESULTSTORE_invocation_id"
    RESULTSTORE_PARENT_BUILD_LINK="$RESULTSTORE_URL/$RESULTSTORE_invocation_id"
    RESULTSTORE_SIBLING_BUILDS_LINK="$RESULTSTORE_SUB_BUILDS_LINK"
    # LINT.ThenChange(
    #   //build/bazel/scripts/generate_invocation_bazelrc.py:related_invocations_env_vars,
    #   //build/bazel/scripts/rsninja.sh:related_invocations_env_vars
    # )
)

_bazel_command=(
    "${_bazel_env[@]}"
    "${_BAZEL_BIN}"
    "${_BAZEL_PRE_COMMAND_ARGS[@]}"
)
# When "${BAZEL_COMMAND} is empty, do not add it nor _BAZEL_EXTRA_ARGS to _bazel_command,
[[ -n "${_BAZEL_COMMAND}" ]] && _bazel_command+=("${_BAZEL_COMMAND}" "${_BAZEL_EXTRA_ARGS[@]}")

_bazel_command+=(
    "${_BAZEL_POST_COMMAND_ARGS[@]}"
    "${_BAZEL_REST_ARGS[@]}"
)

# Save the final invocation to a log file.
echo "${_bazel_command[*]}" >> "${_BAZEL_INVOCATION_LOG_DIR}/bazel_invocation"

if [[ "${FUCHSIA_BAZEL_PRINT_COMMANDS}" == "1" ]]; then
  # Print to stderr to avoid polluting stdout when command output
  # (e.g. from bazel query) is piped or redirected.
  # LINT.IfChange(bazel_command_prefix)
  _BAZEL_COMMAND_PREFIX="[bazel-command]"
  # LINT.ThenChange(//build/bazel/scripts/build_utils.py:bazel_command_prefix)
  echo >&2 "${_BAZEL_COMMAND_PREFIX} ${_bazel_command[*]}"
fi

# Wait for a process to finish, even if wait is interrupted by signals.
# Returns the exit code of the process.
function wait_for_process_exit() {
  local target_pid="$1"
  local exit_code=0
  # Loop indefinitely to keep waiting for the process to fully exit even if individual
  # wait calls are interrupted by incoming/forwarded signals. Waiting indefinitely
  # is correct because it behaves identically to standard 'wait', ensuring the wrapper
  # does not exit prematurely and leave orphaned zombie/running subprocesses.
  # If the user wishes to terminate forcefully, they can send a second SIGINT (Ctrl-C)
  # which propagates down to the child process group.
  while true; do
    # Try to wait for the child process.
    if wait "${target_pid}"; then
      # If wait succeeded (returned 0), the process exited successfully.
      exit_code=0
      break
    fi
    # If wait returned non-zero, it could be because the child failed or
    # because the wait itself was interrupted by a signal.
    exit_code=$?

    # Verify if the process is actually still alive.
    if ! kill -0 "${target_pid}" 2>/dev/null; then
      # If the child process is no longer running, wait one last time to reap
      # the zombie process and retrieve its actual exit status.
      wait "${target_pid}" && exit_code=0 || exit_code=$?
      break
    fi
    # If the process is still running, the signal merely interrupted our wait
    # call. Loop back and continue waiting.
  done
  return "${exit_code}"
}

# Wait for a command while ignoring signals to ensure the parent outlives the child.
# This prevents the shell from exiting prematurely and orphaning backgrounded
# subprocesses during a signal (like Ctrl-C).
function wait-ignoring-signals {
  local child_pid=""
  local sig_count=0

  # Acknowledge signals and forward them to the child.
  function _signal_acknowledgement_handler {
    local sig="$1"
    sig_count=$((sig_count + 1))

    if [[ $sig_count -eq 1 ]]; then
      msg "Received ${sig}. Forwarding to child and waiting for graceful shutdown..."
    else
      msg "Received ${sig} again (${sig_count}). Still waiting for cleanup..."
    fi

    if [[ -n "${child_pid}" ]]; then
       # Signal the child process group. Since we are likely in a separate
       # process group from the TTY (due to wrappers above us), we must
       # explicitly forward signals to the sub-tree.
       kill -"${sig}" "-${child_pid}" 2>/dev/null || true
    fi
  }
  trap '_signal_acknowledgement_handler SIGINT' INT
  trap '_signal_acknowledgement_handler SIGTERM' TERM
  trap '_signal_acknowledgement_handler SIGHUP' HUP

  # Run the command in its own process group.
  # This makes it easier to signal the entire sub-tree (including Bazel).
  set -m
  "$@" &
  child_pid=$!
  set +m

  local status=0
  wait_for_process_exit "${child_pid}" && status=0 || status=$?

  trap - INT TERM HUP
  return "$status"
}

cd "${_BAZEL_WORKSPACE}" && wait-ignoring-signals "${_bazel_command[@]}"
