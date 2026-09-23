# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Lack of shebang in this file is intentional

# This file can be sourced multiple times when running certain tests.
# Guard against it to avoid errors about redefining readonly variables.
if [[ -z "${lib_vars_sh_sourced:-}" ]]; then
readonly lib_vars_sh_sourced=true

if [[ -n "${ZSH_VERSION:-}" ]]; then
  # shellcheck disable=SC2296,SC2298
  devshell_lib_dir=${${(%):-%x}:a:h}
  FUCHSIA_DIR="$(dirname "$(dirname "$(dirname "${devshell_lib_dir}")")")"
else
  # NOTE: Replace use of dirname with BASH substitutions to save 20ms of
  # startup time for all fx commands.
  #
  # Equivalent to "$(dirname ${BASH_SOURCE[0]"}) but 350x faster.
  # Exception: BASH_SOURCE[0] will be the script name when invoked directly
  # from Bash (e.g. cd tools/devshell/lib && bash vars.sh). In this case
  # the substitution will not change anything, so provide fallback case.
  devshell_lib_dir="${BASH_SOURCE[0]%/*}"
  if [[ "${devshell_lib_dir}" == "${BASH_SOURCE[0]}" ]]; then
    devshell_lib_dir="."
  fi
  # Get absolute path.
  devshell_lib_dir="$(cd "${devshell_lib_dir}" >/dev/null 2>&1 && pwd)"
  # Compute absolute path to $devshell_lib_dir/../../..
  FUCHSIA_DIR="${devshell_lib_dir}"
  FUCHSIA_DIR="${FUCHSIA_DIR%/*}"
  FUCHSIA_DIR="${FUCHSIA_DIR%/*}"
  FUCHSIA_DIR="${FUCHSIA_DIR%/*}"
fi

export FUCHSIA_DIR
export FUCHSIA_OUT_DIR="${FUCHSIA_OUT_DIR:-${FUCHSIA_DIR}/out}"

# For all fx commands, we recommend pointing Python's cache dir outside of
# the source tree, so that it is possible to mount the source tree read-only.
# Honor PYTHONPYCACHEPREFIX from the user's environment over the defaults
# chosen here.
# Reasonable choices include: $HOME/.cache/__pycache__
if [[ -z "${PYTHONPYCACHEPREFIX:-}" ]]; then
  if [[ -f "${FUCHSIA_DIR}/.fx-build-dir" ]]; then
    # If there is a build-dir, use it.  This will be cleaned by 'fx clean'.
    _fx_build_dir="$(<"${FUCHSIA_DIR}/.fx-build-dir")"
    if [[ "${_fx_build_dir:0:1}" != "/" ]]; then
      _fx_build_dir="${FUCHSIA_DIR}/${_fx_build_dir}"
    fi
    export PYTHONPYCACHEPREFIX="${_fx_build_dir}/__pycache__"
    unset _fx_build_dir
  else
    # Without a build-dir, use the out/ dir as a fallback.
    # This location has the benefit of cache-sharing across all build dirs.
    export PYTHONPYCACHEPREFIX="${FUCHSIA_OUT_DIR}/__pycache__"
  fi
fi

# shellcheck source=/dev/null
source "${devshell_lib_dir}/platform.sh"
# shellcheck source=/dev/null
source "${devshell_lib_dir}/fx-cmd-locator.sh"
# shellcheck source=/dev/null
source "${devshell_lib_dir}/fx-optional-features.sh"
# shellcheck source=/dev/null
source "${devshell_lib_dir}/generate-ssh-config.sh"
# shellcheck source=/dev/null
source "${devshell_lib_dir}/is_invoked_by_agent.sh"
unset devshell_lib_dir

# Subcommands can use this directory to cache artifacts and state that should
# persist between runs. //scripts/fx ensures that it exists.
#
# fx commands that make use of this directory should include the command name
# in the names of any cached artifacts to make naming collisions less likely.
export FX_CACHE_DIR="${FUCHSIA_DIR}/.fx"

# If the fx config does not exist, create it.
FX_CONFIG_DIR="${FX_CACHE_DIR}/config"
[ -d "${FX_CONFIG_DIR}" ] || mkdir -p "${FX_CONFIG_DIR}"

# This allows LLVM utilities to perform debuginfod lookups for public artifacts.
# See https://sourceware.org/elfutils/Debuginfod.html.
# TODO(111990): Replace this with a local authenticating proxy to support access
#   control.
public_url="https://storage.googleapis.com/fuchsia-artifacts"
if [[ "$DEBUGINFOD_URLS" != *"$public_url"* ]]; then
  export DEBUGINFOD_URLS="${DEBUGINFOD_URLS:+$DEBUGINFOD_URLS }$public_url"
fi
unset public_url

if [[ "${FUCHSIA_DEVSHELL_VERBOSITY:-0}" -eq 1 ]]; then
  set -x
fi

# If build profiling is enabled, collect system stats during build,
# including CPU, memory, disk I/O...
# Since our system_profiler implementation is Linux-specific (relying on direct
# /proc reads), we only auto-enable it on Linux host platforms.
if [[ "$OSTYPE" =~ "linux" ]]; then
  BUILD_PROFILE_ENABLED=1
else
  BUILD_PROFILE_ENABLED=0
fi
readonly fx_build_profile_config="${FX_CONFIG_DIR}/build-profile"
readonly fx_build_profile_config_old="${FUCHSIA_DIR}/.fx-build-profile-config"
if [[ -f "$fx_build_profile_config_old" ]]; then
  fx-info "Moving $fx_build_profile_config_old to new location $fx_build_profile_config.  No further action is necessary."
  mv "$fx_build_profile_config_old" "$fx_build_profile_config"
fi
if [[ -f "$fx_build_profile_config" ]]; then
  # shellcheck source=/dev/null
  source "$fx_build_profile_config"
  # This sets BUILD_PROFILE_ENABLED to 0 or 1.
fi

# Always create a ninja build trace file.
# This path is relative to the the ninja -C dir ($FUCHSIA_BUILD_DIR).
readonly NINJA_BUILD_TRACE_FILE="ninja_build_trace.json.gz"

# Output action metrics to this JSON file.
readonly NINJA_ACTION_METRICS_FILE="ninja_action_metrics.json"

# Record the set if inputs that triggered some build actions.
readonly NINJA_DIRTY_SOURCES_FILE="ninja_dirty_sources.log"

# Unified standalone build script that can enable RBE, profiling, ResultStore...
readonly main_build_script="${FUCHSIA_DIR}/build/scripts/main_build.py"

# If ResultStore is enabled, wrap builds with ResultStore tools.
RESULTSTORE_ENABLED=0
readonly fx_resultstore_config="${FX_CONFIG_DIR}/resultstore"
if [[ -f "$fx_resultstore_config" ]]; then
  # shellcheck source=/dev/null
  source "$fx_resultstore_config"
  # This sets RESULTSTORE_ENABLED to 0 or 1.
fi

date="$(date +%Y%m%d-%H%M%S)"
readonly date

readonly jq="$PREBUILT_JQ"


# fx-command-stdout-to-array runs a command and stores its standard output
# into an array (expecting one item per line, preserving spaces).
# $1: variable name
# $2+: command arguments.
if [[ "${BASH_VERSION}" =~ ^3 ]]; then
  # MacOS comes with Bash 3.x which doesn't have readarray at all
  # so read one line at a time.
  function fx-command-stdout-to-array {
    local varname="$1"
    local line output
    shift
    output="$("$@")"
    while IFS= read -r line; do
      eval "${varname}+=(\"${line}\")"
    done <<< "${output}"
  }
else
  function fx-command-stdout-to-array {
    local varname="$1"
    shift
    readarray -t "${varname}" <<< "$("$@")"
  }
fi

readonly BUILD_LOGS_DIR=_build_logs
readonly build_logs_root="${FUCHSIA_OUT_DIR}/${BUILD_LOGS_DIR}"

# LINT.IfChange(build_log_dir_structure)
# Prints the path to the most recent build log directory
# created by $main_build_script.
function fx-get-last-build-log-dir() {
  fx-build-dir-if-present || return
  local -r out_dir_name="$(basename "${FUCHSIA_BUILD_DIR}")"
  local -r log_dir_base="${build_logs_root}/${out_dir_name}"
  if [[ -d "$log_dir_base" ]]; then
    # shellcheck disable=SC2012
    local -r recent_log_dir="$(ls -1 -d "${log_dir_base}"/build.* | tail -n 1)"
    if [[ -d "$recent_log_dir" ]]; then
      echo "$recent_log_dir"
      return
    fi
  fi
}
# LINT.ThenChange(//build/scripts/main_build.py:build_log_dir_structure)

# Wait for a command while ignoring signals to ensure the parent outlives the child.
# This prevents the shell from exiting prematurely and orphaning backgrounded
# subprocesses during a signal (like Ctrl-C).
#
# Successive signals will continue to reach the child as long as it is alive.
#
# Arguments:
#   $1: caller name (used in acknowledgement message)
#   $@: command and arguments
#
function fx-wait-ignoring-signals {
  local caller_name="$1"
  shift

  # Save original trap for signals.
  local orig_trap
  orig_trap=$(trap -p INT TERM HUP)

  local sig_count=0
  # Acknowledge signals but stay alive while waiting.
  function _fx_signal_acknowledgement_handler {
    local sig="$1"
    sig_count=$((sig_count + 1))
    if [[ $sig_count -eq 1 ]]; then
      echo >&2 "[${caller_name}] Received ${sig}. Waiting for command to shut down gracefully..."
    else
      echo >&2 "[${caller_name}] Received ${sig} again (${sig_count}). Still waiting for cleanup..."
    fi
  }
  trap '_fx_signal_acknowledgement_handler SIGINT' INT
  trap '_fx_signal_acknowledgement_handler SIGTERM' TERM
  trap '_fx_signal_acknowledgement_handler SIGHUP' HUP

  # Run the command in a subshell that restores default signal dispositions.
  # This ensures the child doesn't inherit the 'ignore' disposition,
  # which would prevent high-level languages (Python/Go) from seeing
  # the signal.
  ( trap - INT TERM HUP ; exec "$@" )
  local status=$?

  # Restore original traps immediately so the shell is responsive during
  # its own exit and cleanup phase.
  if [[ -n "$orig_trap" ]]; then
    eval "$orig_trap"
  else
    trap - INT TERM HUP
  fi

  return "$status"
}

# Returns 0 if RBE is enabled for this build.
# NOTE: this function depends on FUCHSIA_BUILD_DIR which is set only after
# initialization.
# The cached version of this function is 'fx-rbe-enabled', below.
function recheck-fx-rbe-enabled {
  # This function is called during tests without a build directory.
  # Returns 1 to indicate that RBE is not enabled.
  fx-build-dir-if-present || return 1

  # This RBE settings file is created at GN gen time.
  local -r rbe_settings_file="${FUCHSIA_BUILD_DIR}/rbe_settings.json"

  # Check to see if the rbe settings indicate that the reproxy wrapper is
  # needed.
  # shellcheck disable=SC2207
  local -a needs_reproxy
  fx-command-stdout-to-array needs_reproxy "$jq" '-r' '.final.needs_reproxy' "${rbe_settings_file}"
  if [[ "${needs_reproxy[0]}" != "true" ]]; then
    return 1
  fi
}

_fx_rbe_enabled_cache_var=
function fx-rbe-enabled {
  if [[ -z "$_fx_rbe_enabled_cache_var" ]]; then
     recheck-fx-rbe-enabled
     # Cache the return status.
     _fx_rbe_enabled_cache_var="$?"
  fi
  return "$_fx_rbe_enabled_cache_var"
}

# Returns 0 if the build is configured to use authenticated services.
# The value of this function is cached by 'fx-build-needs-auth'.
function recheck-fx-build-needs-auth() {
  # For testing-only, return 1 to indicate that authentication is not needed.
  fx-build-dir-if-present || return 1

  # The ResultStore service requires authentication.
  if [[ "${RESULTSTORE_ENABLED}" -eq 1 ]]; then
    return 0
  fi

  # This RBE settings file is created at GN gen time.
  local -r rbe_settings_file="${FUCHSIA_BUILD_DIR}/rbe_settings.json"

  # Return 0 if authentication is needed for build remote services,
  # otherwise return 1.
  local -a needs_auth
  fx-command-stdout-to-array needs_auth "$jq" '-r' '.final.needs_auth' "${rbe_settings_file}"
  if [[ "${needs_auth[0]}" != "true" ]]; then
    return 1
  fi
}

_fx_build_needs_auth_cache_var=
function fx-build-needs-auth {
  if [[ -z "$_fx_build_needs_auth_cache_var" ]]; then
     recheck-fx-build-needs-auth
     # Cache the return status.
     _fx_build_needs_auth_cache_var="$?"
  fi
  return "$_fx_build_needs_auth_cache_var"
}

# fx-is-stderr-tty exits with success if stderr is a tty.
function fx-is-stderr-tty {
  [[ -t 2 ]]
}

# fx-info prints a line to stderr with a blue INFO: prefix.
function fx-info {
  if fx-is-stderr-tty; then
    echo -e >&2 "\033[1;34mINFO:\033[0m $*"
  else
    echo -e >&2 "INFO: $*"
  fi
}

# fx-warn prints a line to stderr with a yellow WARNING: prefix.
function fx-warn {
  if fx-is-stderr-tty; then
    echo -e >&2 "\033[1;33mWARNING:\033[0m $*"
  else
    echo -e >&2 "WARNING: $*"
  fi
}

# fx-error prints a line to stderr with a red ERROR: prefix.
function fx-error {
  if fx-is-stderr-tty; then
    echo -e >&2 "\033[1;31mERROR:\033[0m $*"
  else
    echo -e >&2 "ERROR: $*"
  fi
}

# fx-fatal prints a line to stderr with a red FATAL: prefix then aborts the current script.
# Only use this for important assertion failures, and provide user-actionable instructions
# in the error message when that happens!
function fx-fatal {
  if fx-is-stderr-tty; then
    echo -e >&2 "\033[1;31mFATAL:\033[0m $*"
  else
    echo -e >&2 "FATAL: $*"
  fi
  exit 1
}

function fx-gn {
  "${PREBUILT_GN}" "$@"
}

function fx-is-bringup {
  grep '^[^#]*import("//products/bringup.gni")' "${FUCHSIA_BUILD_DIR}/args.gn" >/dev/null 2>&1
}

function fx-fail-if-main-pb-is-not-set {
  # Read the three relevant args all at once.
  local -a values
  mapfile -t values < <(fx-command-run jq -r \
    '.main_pb_label // "",
     .bazel_product_bundle_full // "",
     .bazel_product_bundle_prefix // ""' \
    "${FUCHSIA_BUILD_DIR}/args.json")

  # Pull the args out of the array and into their own variables.
  local main_pb_label bazel_product_bundle_full bazel_product_bundle_prefix
  main_pb_label="${values[0]}"
  bazel_product_bundle_full="${values[1]}"
  bazel_product_bundle_prefix="${values[2]}"

  # Fail if this is a multi-product build and main_pb_label is not set.
  if [[ -z "${bazel_product_bundle_full}" && -z "${bazel_product_bundle_prefix}" && -z "${main_pb_label}" ]]; then
    fx-error "The 'main_pb_label' GN argument is not set. Please set it with: fx set-main-pb <name>"
    exit 1
  fi
}

function fx-regenerator {
  PYTHONPYCACHEPREFIX="${PYTHONPYCACHEPREFIX}" \
  fx-wait-ignoring-signals "fx-regenerator" \
    "${FUCHSIA_DIR}/build/regenerator" \
    --fuchsia-dir="${FUCHSIA_DIR}" \
    --fuchsia-build-dir="${FUCHSIA_BUILD_DIR}" \
    "$@"
}

# shellcheck disable=SC2120
function fx-gen {
  fx-regenerator "$@"
}

function fx-gn-args {
  fx-regenerator --update-args "$@"
}

function fx-build-config-load {
  # Paths are relative to FUCHSIA_DIR unless they're absolute paths.
  if [[ "${FUCHSIA_BUILD_DIR:0:1}" != "/" ]]; then
    FUCHSIA_BUILD_DIR="${FUCHSIA_DIR}/${FUCHSIA_BUILD_DIR}"
  fi

  if [[ ! -f "${FUCHSIA_BUILD_DIR}/fx.config" ]]; then
    if [[ ! -f "${FUCHSIA_BUILD_DIR}/args.gn" ]]; then
      fx-error "Build directory missing or removed. (${FUCHSIA_BUILD_DIR})"
      fx-error "run \"fx set\", or specify a build dir with --dir or \"fx use\""
      return 1
    fi

    fx-gen || return 1
  fi

  # shellcheck source=/dev/null
  if ! source "${FUCHSIA_BUILD_DIR}/fx.config"; then
    fx-error "Sourcing ${FUCHSIA_BUILD_DIR}/fx.config caused internal error"
    if [[ -f "${FUCHSIA_BUILD_DIR}/args.gn" ]]; then
      fx-error "Try running 'fx gen' to regenerate your build configuration."
    else
      fx-error "Try running 'fx set' to generate a new build configuration."
    fi
    return 1
  fi

  export FUCHSIA_BUILD_DIR FUCHSIA_ARCH

  if [[ "${HOST_OUT_DIR:0:1}" != "/" ]]; then
    HOST_OUT_DIR="${FUCHSIA_BUILD_DIR}/${HOST_OUT_DIR}"
  fi

  fx-export-default-target

  return 0
}

function fx-export-default-target {
  # Set the device specified at the build directory level, if any.
  if [[ -z "${FUCHSIA_NODENAME_SET_BY_FX_FLAG}" && ( -z "${FUCHSIA_NODENAME}" || "${FUCHSIA_NODENAME_IS_FROM_FILE}" == "true" ) ]]; then
    if [[ -f "${FUCHSIA_BUILD_DIR}.device" ]]; then
      FUCHSIA_NODENAME="$(<"${FUCHSIA_BUILD_DIR}.device")"
      export FUCHSIA_NODENAME_IS_FROM_FILE="true"
    fi
    export FUCHSIA_NODENAME
  fi
}

# Forces the command to fail if a user specifies the -t flag
function fx-fail-if-device-specified {
  if [[ -n "${FUCHSIA_NODENAME_SET_BY_FX_FLAG}" ]]; then
    fx-error "The -t flag is not supported when calling this function"
    fx-error "Please remove the -t or --target flag from your command."
    exit 1
  fi
}

# Sets FUCHSIA_BUILD_DIR once, to an absolute path.
function fx-build-dir-if-present {
  if [[ -n "${FUCHSIA_BUILD_DIR:-}" ]]; then
    # already set by this function earlier
    return 0
  elif [[ -n "${_FX_BUILD_DIR:-}" ]]; then
    export FUCHSIA_BUILD_DIR="${_FX_BUILD_DIR}"
    # This can be set by --dir.
    # Unset to prevent subprocess from acting on it again.
    unset _FX_BUILD_DIR
  else
    if [[ ! -f "${FUCHSIA_DIR}/.fx-build-dir" ]]; then
      return 1
    fi

    # .fx-build-dir contains $FUCHSIA_BUILD_DIR
    FUCHSIA_BUILD_DIR="$(<"${FUCHSIA_DIR}/.fx-build-dir")"
    if [[ -z "${FUCHSIA_BUILD_DIR}" ]]; then
      return 1
    fi
    # Paths are relative to FUCHSIA_DIR unless they're absolute paths.
    if [[ "${FUCHSIA_BUILD_DIR:0:1}" != "/" ]]; then
      FUCHSIA_BUILD_DIR="${FUCHSIA_DIR}/${FUCHSIA_BUILD_DIR}"
    fi
  fi
  return 0
}

function fx-config-read {
  if ! fx-build-dir-if-present; then
    fx-error "No build directory found."
    fx-error "Run \"fx set\" to create a new build directory, or specify one with --dir"
    exit 1
  fi

  fx-build-config-load || exit $?

  # LINT.IfChange(build_lock)
  _FX_LOCK_FILE="${FUCHSIA_BUILD_DIR}.build_lock"
  # LINT.ThenChange(//build/scripts/main_build.py:build_lock)
}

# Evaluates $@ if $FUCHSIA_NODENAME or $FUCHSIA_DEVICE_ADDR have been set by the
# user outside of `fx set-device` or `fx -t|--target`.
#
# Argument expressions are evaluated with the following environment variables
# provided:
#  - ENV_VAR_NAMES: A human readable message fragment of the environment
#    variable names that are overriding the default target.
#    Example: '$FUCHSIA_NODENAME and $FUCHSIA_DEVICE_ADDR'
#  - ENV_VARS: A human readable message fragment of the environment variable
#    key-value pairs that are overriding the default target.
#    Example: '$FUCHSIA_NODENAME="foo" and $FUCHSIA_DEVICE_ADDR="bar"'
#
# Examples:
# > fx-if-target-set-by-env echo '$ENV_VARS overrides the default target.'
#
# > fx-if-target-set-by-env echo '$ENV_VARS overrides the default target.'
# $FUCHSIA_NODENAME="foo" overrides the default target.
#
# > fx-if-target-set-by-env echo '$ENV_VAR_NAMES overrides the default target.'
# $FUCHSIA_NODENAME and $FUCHSIA_DEVICE_ADDR overrides the default target.
_FUCHSIA_NODENAME_FROM_PARENT="${FUCHSIA_NODENAME:-}"
function fx-if-target-set-by-env {
  if [[ -n "${_FUCHSIA_NODENAME_FROM_PARENT}" && -n "${FUCHSIA_DEVICE_ADDR}" ]]; then
    ENV_VAR_NAMES="\$FUCHSIA_NODENAME and \$FUCHSIA_DEVICE_ADDR" \
    ENV_VARS="\$FUCHSIA_NODENAME=\"${_FUCHSIA_NODENAME_FROM_PARENT}\" and \$FUCHSIA_DEVICE_ADDR=\"${FUCHSIA_DEVICE_ADDR}\"" \
    "$@"
  elif [[ -n "${_FUCHSIA_NODENAME_FROM_PARENT}" ]]; then
    ENV_VAR_NAMES="\$FUCHSIA_NODENAME" \
    ENV_VARS="\$FUCHSIA_NODENAME=\"${_FUCHSIA_NODENAME_FROM_PARENT}\"" \
    "$@"
  elif [[ -n "${FUCHSIA_DEVICE_ADDR}" ]]; then
    ENV_VAR_NAMES="\$FUCHSIA_DEVICE_ADDR" \
    ENV_VARS="\$FUCHSIA_DEVICE_ADDR=\"${FUCHSIA_DEVICE_ADDR}\"" \
    "$@"
  fi
}

function fx-change-build-dir {
  local build_dir="$1"

  local -r tempfile="$(mktemp)"
  echo "${build_dir}" > "${tempfile}"
  mv -f "${tempfile}" "${FUCHSIA_DIR}/.fx-build-dir"

  # Now update the environment and root-symlinked build artifacts to reflect
  # the change.
  fx-config-read

  fx-regenerator "--symlinks-only"
}

function ffx-default-repository-name {
    # Use the build directory's name by default. Note that package URLs are not
    # allowed to have uppercase letters, underscores, or dots (which cause invalid
    # IPv4 parsing if ending in digits), so sanitize them to lowercase hyphens.
    basename "${FUCHSIA_BUILD_DIR}" | tr '[:upper:]_.' '[:lower:]--'
}

# Runs a jq command against an existing file that will edit it, taking care
# of piping output to a tempfile and replacing it if jq succeeds.
# `_jq_edit <jq args> path/to/file.json`
#
# Note that the last argument is expected to be the filename being edited, to
# match the normal argument order of jq, but unlike with jq it's required.
function _jq_edit {
  # Take the last argument as the path to the edited file.
  # shellcheck disable=SC2124
  local json_file="${@: -1}"

  local -r tempfile="$(mktemp)"
  if fx-command-run jq "$@" > "${tempfile}" ; then
    mv -f "${tempfile}" "${json_file}"
    return 0
  else
    return $?
  fi
}

# Set a configuration value in a build config file:
# `json-config-set path/to/file.json path.to.value "value"`
function json-config-set {
  local json_file="$1"
  local path="$2"
  local value="$3"

  # There needs to be a file there in the first place, so if there isn't one
  # create one with an empty object for jq to update.
  if ! [[ -f "${json_file}" ]] ; then
    echo "{}" > "${json_file}"
  fi

  _jq_edit -e -cS --arg value "${value}" ".${path} = \$value" "${json_file}"
  return $?
}

# Remove a configuration value in a build config file:
# `json-config-del path/to/file.json path.to.value`
#
# Will return a non-zero status code if the file or value did not already
# exist.
function json-config-del {
  local json_file="$1"
  local path="$2"

  if [[ -f "${json_file}" ]] ; then
    # check the path exists, delete it if so, exit with error code otherwise.
    _jq_edit -e -cS "if .${path} then del(.${path}) else empty | halt_error(1) end" "${json_file}"
    return $?
  else
    return 1
  fi
}

# Get a configuration value in a build config file:
# `json-config-get path/to/file.json path.to.value`
#
# Returns 1 and prints an empty line if the file does not exist or
# the path given was not already set.
function json-config-get {
  local json_file="$1"
  local path="$2"

  if ! [[ -f "${json_file}" ]] ; then
    echo
    return 1
  else
    fx-command-run jq -e -r ".${path} | values" "${json_file}"
  fi
}

function get-device-raw {
  fx-config-read
  local device=""
  # Suppress the unset default target message from stderr.
  device="$(ffx target default get 2>/dev/null)"

  if ! is-valid-device "${device}"; then
    fx-error "Invalid device name or address: '${device}'. Some valid examples are:
      strut-wind-ahead-turf, 192.168.3.1:8022, [fe80::7:8%eth0], [fe80::7:8%eth0]:5222, [::1]:22, serial:EM-C0B2F2D96, uart:/dev/ttyUSB0"
    exit 1
  fi
  echo "${device}"
}

function is-valid-device {
  local device="$1"
  if [[ -n "${device}" ]] \
      && ! _looks_like_ipv4 "${device}" \
      && ! _looks_like_ipv6 "${device}" \
      && ! _looks_like_hostname "${device}" \
      && ! _looks_like_usb_or_vsock "${device}" \
      && ! _looks_like_serial_or_id "${device}" \
      && ! _looks_like_uart "${device}"; then
    return 1
  fi
}

# Shared among a few subcommands to configure and identify a remote forward
# target for a device.
export _FX_REMOTE_WORKFLOW_DEVICE_ADDR='[::1]:8022'

function is-remote-workflow-device {
  [[ $(get-device-raw 2>/dev/null) == "${_FX_REMOTE_WORKFLOW_DEVICE_ADDR}" ]]
}

# fx-export-device-address is "public API" to commands that wish to
# have the exported variables set.
function fx-export-device-address {
  FX_DEVICE_NAME="$(get-device-name)"
  FX_DEVICE_ADDR="$(get-fuchsia-device-addr)"
  FX_SSH_ADDR="$(get-device-addr-resource)"
  FX_SSH_PORT="$(get-device-ssh-port)"
  export FX_DEVICE_NAME FX_DEVICE_ADDR FX_SSH_ADDR FX_SSH_PORT
}

function get-device-ssh-port {
  local device
  device="$(get-device-raw)" || exit $?
  local addr_port="$(fx-target-finder-resolve "${device}")"
  local port=""
  # If there's more than one device returned by `ffx target list`,
  # return an empty port
  if [[ "$addr_port" != *$'\n'* ]] ;  then
    # extract port, if present
    if [[ "${addr_port}" =~ :([0-9]+)$ ]]; then
      port="${BASH_REMATCH[1]}"
    fi
  fi
  echo "${port}"
}

function get-device-name {
  local device
  device="$(get-device-raw)" || exit $?
  # remove ssh port if present
  if _looks_like_hostname "${device}" || _looks_like_ipv4 "${device}"; then
    if [[ "${device}" =~ ^(.*):[0-9]{1,5}$ ]]; then
      device="${BASH_REMATCH[1]}"
    fi
  elif _looks_like_ipv6 "${device}"; then
    # parse the address into parts.
    local expression='^\[([0-9a-fA-F:]+(%[0-9a-zA-Z-]{1,})?)\](:[0-9]{1,5})?$'
    if ! [[ "$device" =~ ${expression} ]]; then
      # try again but wrap the arg in "[]"
      local wrapped="[${device}]"
      if ! [[ "$device" =~ ${expression} ]]; then
        echo "$device"
        return
      fi
    fi
    device="[${BASH_REMATCH[1]}]"

  fi
  echo "${device}"
}

function _looks_like_usb_or_vsock {
  [[ "$1" =~ ^(usb|vsock):cid:[0-9]+$ ]] || return 1
}

function _looks_like_serial_or_id {
  [[ "$1" =~ ^(serial|id):[A-Za-z0-9_.-]+$ ]] || return 1
}

# Checks if the target string matches the UART address format: uart:<path-or-id>
# Returns 0 if the argument matches, 1 otherwise.
function _looks_like_uart {
  [[ "$1" =~ ^uart:[^[:space:]]+$ ]] || return 1
}

function _looks_like_hostname {
  _looks_like_serial_or_id "$1" && return 1
  _looks_like_uart "$1" && return 1
  [[ "$1" =~ ^([a-z0-9][.a-z0-9-]*)?(:[0-9]{1,5})?$ ]] || return 1
}

function _looks_like_ipv4 {
  [[ "$1" =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}(:[0-9]{1,5})?$ ]] || return 1
}

function _looks_like_ipv6 {
  local expression
  expression='^\[([0-9a-fA-F:]+(%[0-9a-zA-Z-]{1,})?)\](:[0-9]{1,5})?$'
  if ! [[ "$1" =~ ${expression} ]]; then
    # try again but wrap the arg in "[]"
    local wrapped
    wrapped="[${1}]"
    [[ "${wrapped}" =~ ${expression} ]] || return 1
    # Okay check that the address is not just a bare port
    ! [[ "$1" =~ ^:[0-9a-fA-F]+$ ]] || return 1
    # Okay check that the address is not just a host name
    ! [[ "$1" =~ ^[0-9a-fA-F\.]+$ ]] || return 1
  fi
  local colons="${BASH_REMATCH[1]//[^:]}"
  # validate that there are no more than 7 colons
  [[ "${#colons}" -le 7 ]] || return 1
}

function _print_ssh_warning {
  fx-warn "Cannot load device SSH credentials. $*"
  fx-warn "Run 'tools/ssh-keys/gen-ssh-keys.sh' to regenerate."
}

# Checks if default SSH keys are missing.
#
# The argument specifies which line of the manifest to retrieve and verify for
# existence.
#
# "key": The SSH identity file (private key). Append ".pub" for the
#   corresponding public key.
# "auth": The authorized_keys file.
function _get-ssh-key {
  local -r _SSH_MANIFEST="${FUCHSIA_DIR}/.fx-ssh-path"

  local filepath
  local -r which="$1"
  if [[ ! "${which}" =~ ^(key|auth)$ ]]; then
    fx-error "_get-ssh-key: invalid argument '$1'. Must be either 'key' or 'auth'"
    exit 1
  fi

  if [[ ! -f "${_SSH_MANIFEST}" ]]; then
    _print_ssh_warning "File not found: ${_SSH_MANIFEST}."
    return 1
  fi

  # Set -r flag to avoid interpreting backslashes as escape characters, e.g. it
  # won't convert "\n" to a newline character.
  { read -r privkey && read -r authkey; } < "${_SSH_MANIFEST}"

  if [[ -z $privkey || -z $authkey ]]; then
    _print_ssh_warning "Manifest file ${_SSH_MANIFEST} is malformed."
    return 1
  fi

  if [[ $which == "auth" ]]; then
    filepath="${authkey}"
  elif [[ $which == "key" ]]; then
    filepath="${privkey}"
  fi

  echo "${filepath}"

  if [[ ! -f "${filepath}" ]]; then
    _print_ssh_warning "File not found: ${filepath}."
    return 1
  fi

  return 0
}

# Invoke the ffx tool. This function invokes ffx via the tools/devshell/ffx
# wrapper that passes configuration information such as default target and build
# directory locations to the ffx tool as needed to provide a seamless fx/ffx
# integration.
function ffx {
  fx-command-run ffx --config fuchsia.analytics.ffx_invoker=fx "$@"
}

function _is-ffx-built {
  fx-build-dir-if-present && [[ -x "${FUCHSIA_BUILD_DIR}/host-tools/ffx" ]]
}

# Prints path to the default SSH key. These credentials are created
# and configured via ffx.
#
# The corresponding public key is stored in "$(get-ssh-privkey).pub".
function get-ssh-privkey {
  # If ffx is not built yet, check for existing keys directly to avoid
  # triggering a slow build of ffx for commands that only need SSH access.
  if ! _is-ffx-built; then
    if [[ -n "${FUCHSIA_SSH_KEY:-}" && -f "${FUCHSIA_SSH_KEY}" ]]; then
      echo "${FUCHSIA_SSH_KEY}"
      return 0
    elif [[ -f "${HOME}/.ssh/fuchsia_ed25519" ]]; then
      echo "${HOME}/.ssh/fuchsia_ed25519"
      return 0
    fi
  fi

  init="$(fx-command-run ffx --config fuchsia.analytics.ffx_invoker=fx config check-ssh-keys --create)"
  RESULT=$?
  if [ $RESULT -ne 0 ]; then
    fx-error "$init"
    return 1
  fi
  val="$(fx-command-run ffx --config fuchsia.analytics.ffx_invoker=fx config get --process file ssh.priv)"
  temp="${val%\"}"
  authkeys="${temp#\"}"
  echo "${authkeys}"
}

# Prints path to the default authorized_keys to include on Fuchsia devices.
function get-ssh-authkeys {
  # If ffx is not built yet, check for existing keys directly to avoid
  # triggering a slow build of ffx for commands that only need SSH access.
  if ! _is-ffx-built; then
    if [[ -n "${FUCHSIA_AUTHORIZED_KEYS:-}" && -f "${FUCHSIA_AUTHORIZED_KEYS}" ]]; then
      echo "${FUCHSIA_AUTHORIZED_KEYS}"
      return 0
    elif [[ -f "${HOME}/.ssh/fuchsia_authorized_keys" ]]; then
      echo "${HOME}/.ssh/fuchsia_authorized_keys"
      return 0
    fi
  fi

  init="$(fx-command-run ffx --config fuchsia.analytics.ffx_invoker=fx config check-ssh-keys --create)"
  RESULT=$?
  if [ $RESULT -ne 0 ]; then
    fx-error "$init"
    return 1
  fi
  val="$(fx-command-run ffx --config fuchsia.analytics.ffx_invoker=fx config get --process file ssh.pub)"
  temp="${val%\"}"
  authkeys="${temp#\"}"
  echo "${authkeys}"
}

# Checks the ssh_config file exists and references the private key, otherwise
# (re)creates it
function check-ssh-config {
  fx-build-dir-if-present || return 1
  privkey="$(get-ssh-privkey)"
  conffile="${FUCHSIA_BUILD_DIR}/ssh-keys/ssh_config"

  local identities_only="false"
  if [[ -x "${FUCHSIA_BUILD_DIR}/host-tools/ffx" ]]; then
    identities_only="$("${FUCHSIA_BUILD_DIR}/host-tools/ffx" config get ssh.identities_only 2>/dev/null || echo "false")"
  fi
  if [[ "$identities_only" != "true" ]]; then
    identities_only="false"
  fi
  local identities_only_mapped="no"
  if [[ "$identities_only" == "true" ]]; then
    identities_only_mapped="yes"
  fi

  if [[ ! -f "${conffile}" ]] || ! grep -q "IdentityFile\s*$privkey" "$conffile" || ! grep -q "IdentitiesOnly $identities_only_mapped" "$conffile"; then
    generate-ssh-config "$privkey" "$conffile" "$identities_only_mapped"
    if [[ $? -ne 0 || ! -f "${conffile}" ]] || ! grep -q "IdentityFile\s*$privkey" "$conffile"; then
      fx-error "Unexpected error, cannot generate ssh_config: ${conffile}"
      exit 1
    fi
  fi
}

function fx-target-finder-resolve {
  if [[ $# -ne 1 ]]; then
    fx-error "Invalid arguments to fx-target-finder-resolve: [$*]"
    return 1
  fi
  ffx target list --format addresses "$1"
}

function fx-target-finder-list {
  ffx target list --format addresses
}

function fx-target-finder-info {
  ffx target list --format s
}

function fx-target-ssh-address {
  ffx target list --format addresses
}

function multi-device-fail {
  local output devices
  fx-error "Multiple devices found."
  fx-error "Please specify one of the following devices using either \`fx -t <device-name>\` or \`fx set-device <device-name>\`."
  devices="$(fx-target-finder-info)" || {
    code=$?
    fx-error "Device discovery failed with status: $code"
    exit $code
  }
  while IFS="" read -r line; do
    fx-error "\t${line}"
  done < <(printf '%s\n' "${devices}")
  exit 1
}

function remove-address-brackets-and-port {
  local device="$1"

  # Remove any trailing port (e.g. :22, :34567) from the address
  local regex='(.*):[0-9]+$'
  [[ "$device" =~ $regex ]] && device="${BASH_REMATCH[1]}"

  # Treat IPv4 addresses in the device name as an already resolved
  # device address.
  if _looks_like_ipv4 "${device}"; then
    echo "${device}"
    return 0
  fi
  if _looks_like_ipv6 "${device}"; then
    # remove brackets
    device="${device%]}"
    device="${device#[}"
    echo "${device}"
    return 0
  fi
  return 1
}

function get-fuchsia-device-addr {
  fx-config-read
  local device
  device="$(get-device-name)" || exit $?

  addr=$(remove-address-brackets-and-port "${device}")
  if [ $? -eq 0 ]; then
    echo "${addr}"
    return
  fi

  local output devices
  case "$device" in
    "")
        output="$(fx-target-finder-list)" || {
          code=$?
          fx-error "Device discovery failed with status: $code"
          exit $code
        }
        if [[ "$(echo "${output}" | wc -l)" -gt "1" ]]; then
          multi-device-fail
        fi
        addr=$(remove-address-brackets-and-port "${output}")
        echo "${addr}" ;;
     *) output=$(fx-target-finder-resolve "$device")
        addr=$(remove-address-brackets-and-port  "${output}")
        echo "${addr}" ;;
  esac
}

function get-fuchsia-device-port {
  fx-config-read
  local port
  port="$(get-device-ssh-port)" || exit $?

  if [[ -z "${port}" ]]; then
    local device
    device="$(fx-target-ssh-address)" || {
      code=$?
      fx-error "Device discovery failed with status: ${code}"
      exit ${code}
    }
    if [[ "$(echo "${device}" | wc -l)" -gt "1" ]]; then
      multi-device-fail
    fi
    if [[ "${device}" =~ :([0-9]+)$ ]]; then
      port="${BASH_REMATCH[1]}"
    fi
  fi
  echo "${port}"
}

# get-device-addr-resource returns an address that is properly encased
# for resource addressing for tools that expect that. In practical
# terms this just means encasing the address in square brackets if it
# is an ipv6 address. Note: this is not URL safe as-is, use the -url
# variant instead for URL formulation.
function get-device-addr-resource {
  local addr
  addr="$(get-fuchsia-device-addr)" || exit $?
  if _looks_like_ipv4 "${addr}"; then
    echo "${addr}"
    return 0
  fi

  echo "[${addr}]"
}

function get-device-addr-url {
  get-device-addr-resource | sed 's#%#%25#'
}

function fx-command-run {
  local -r command_name="$1"
  local command_path
  # Use an array because the command may be multiple elements, if it comes from
  # a .fx metadata file.
  command_path="$(find_executable "${command_name}")"
  if [[ $? -ne 0 || ! -x "${command_path}" ]]; then
    fx-error "Unknown command ${command_name}"
    exit 1
  fi

  shift
  env FX_CALLER="$0" "${command_path}" "$@"
}

function fx-command-exec {
  local -r command_name="$1"
  local command_path
  # Use an array because the command may be multiple elements, if it comes from
  # a .fx metadata file.
  command_path="$(find_executable "${command_name}")"
  if [[ $? -ne 0 || ! -x "${command_path}" ]]; then
    fx-error "Unknown command ${command_name}"
    exit 1
  fi

  shift
  exec env FX_CALLER="$0" "${command_path}" "$@"
}

function fx-print-command-help {
  local command_path="$1"
  if grep '^## ' "$command_path" > /dev/null; then
    sed -n -e 's/^## //p' -e 's/^##$//p' < "$command_path"
  else
    local -r command_name=$(basename "$command_path" ".fx")
    echo "No help found. Try \`fx $command_name -h\`"
  fi
}

function fx-command-help {
  fx-print-command-help "$0"
  echo -e "\nFor global options, try \`fx help\`."
}


# This function massages arguments to an fx subcommand so that a single
# argument `--switch=value` becomes two arguments `--switch` `value`.
# This lets each subcommand's main function use simpler argument parsing
# while still supporting the preferred `--switch=value` syntax.  It also
# handles the `--help` argument by redirecting to what `fx help command`
# would do.  Because of the complexities of shell quoting and function
# semantics, the only way for this function to yield its results
# reasonably is via a global variable.  FX_ARGV is an array of the
# results.  The standard boilerplate for using this looks like:
#   function main {
#     fx-standard-switches "$@"
#     set -- "${FX_ARGV[@]}"
#     ...
#     }
# Arguments following a `--` are also added to FX_ARGV but not split, as they
# should usually be forwarded as-is to subprocesses.
function fx-standard-switches {
  # In bash 4, this can be `declare -a -g FX_ARGV=()` to be explicit
  # about setting a global array.  But bash 3 (shipped on macOS) does
  # not support the `-g` flag to `declare`.
  FX_ARGV=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" = "--help" || "$1" = "-h" ]]; then
      fx-print-command-help "$0"
      # Exit rather than return, so we bail out of the whole command early.
      exit 0
    elif [[ "$1" == --*=* ]]; then
      # Turn --switch=value into --switch value.
      FX_ARGV+=("${1%%=*}" "${1#*=}")
    elif [[ "$1" == "--" ]]; then
      # Do not parse remaining parameters after --
      FX_ARGV+=("$@")
      return
    else
      FX_ARGV+=("$1")
    fi
    shift
  done
}

function fx-uuid {
  # Emit a uuid string, same as the `uuidgen` tool.
  # Using Python avoids requiring a separate tool.
  env "PYTHONPYCACHEPREFIX=${PYTHONPYCACHEPREFIX}" \
  "${PREBUILT_PYTHON3}" -S -c 'import uuid; print(uuid.uuid4())'
}

function fx-choose-build-concurrency {
  # If any remote execution is enabled (e.g. via RBE),
  # allow ninja to launch many more concurrent actions than what local
  # resources can support.
  if fx-rbe-enabled ; then
    # The recommendation from the Goma team is to use 10*cpu-count for C++.
    local cpus
    cpus="$(fx-cpu-count)"
    echo $((cpus * 10))
  else
    fx-cpu-count
  fi
}

function fx-cpu-count {
  local -r cpu_count=$(getconf _NPROCESSORS_ONLN)
  echo "$cpu_count"
}


# Use a lock file around a command if possible.
# Print a message if the lock isn't immediately entered,
# and block until it is.
function fx-try-locked {
  if [[ -z "${_FX_LOCK_FILE}" ]]; then
    fx-error "fx internal error: attempt to run locked command before fx-config-read"
    exit 1
  fi
  if ! command -v shlock >/dev/null; then
    # Can't lock! Fall back to unlocked operation.
    fx-exit-on-failure "$@"
  elif shlock -f "${_FX_LOCK_FILE}" -p $$; then
    # This will cause a deadlock if any subcommand calls back to fx build,
    # because shlock isn't reentrant by forked processes.
    fx-cmd-locked "$@"
  else
    echo "Locked by ${_FX_LOCK_FILE}..."
    while ! shlock -f "${_FX_LOCK_FILE}" -p $$; do sleep .1; done

    # This message is critical for AI agents to understand when a build
    # is proceeding after acquiring a lock. Do not remove.
    echo "Lock acquired, proceeding with build."

    fx-cmd-locked "$@"
  fi
}

function fx-cmd-locked {
  if [[ -z "${_FX_LOCK_FILE}" ]]; then
    fx-error "fx internal error: attempt to run locked command before fx-config-read"
    exit 1
  fi
  # Exit trap to clean up lock file. Intentionally use the current value of
  # $_FX_LOCK_FILE rather than the value at the time that trap executes to
  # ensure we delete the original file even if the value of $_FX_LOCK_FILE
  # changes for whatever reason.
  trap "[[ -n \"\${_FX_LOCK_FILE}\" ]] && rm -f \"\${_FX_LOCK_FILE}\"" EXIT
  fx-exit-on-failure "$@"
}

function fx-exit-on-failure {
  "$@" || exit $?
}

# Run a Ninja or Bazel command with the right environment, after eventually
# prepending necessary RBE wrapper scripts to it.
#
# Arguments:
#    print_full_cmd   if true, prints the full ninja command line before
#                     executing it
#    command_type     either "ninja" or "bazel"
#
#    build_command    The build command itself.
#
function fx-run-build-command {
  # Note: Check for a bad element in $PATH (empty or '.') is now performed
  # in the python implementation to ensure early failure.

  local print_full_cmd="$1"
  shift 1
  local -a args=(
    "--build-dir" "${FUCHSIA_BUILD_DIR}"
    "--out-dir" "${FUCHSIA_OUT_DIR}"
    "--rbe=$(fx-rbe-enabled && echo true || echo false)"
    "--resultstore=${RESULTSTORE_ENABLED}"
    "--profile=${BUILD_PROFILE_ENABLED}"
    "--tui=${TUI_ENABLED:-0}"

    # MAX_BUILD_CONCURRENCY comes from fx.config, written by `gn gen`.
    "--max-concurrency=${MAX_BUILD_CONCURRENCY:-0}"
  )
  if [[ "${print_full_cmd}" == "true" ]]; then
    args+=("--verbose")
  fi
  if [[ "${FX_BUILD_DRY_RUN}" == "1" ]]; then
    args+=("--dry-run")
  fi
  FX_BUILD_STATUS="${FX_BUILD_STATUS:-1}"
  if [[ "${FX_BUILD_STATUS}" == "0" || ! -t 1 || "${TERM}" == "dumb" ]] || is_invoked_by_agent; then
    args+=("--no-status")
  fi
  args+=(
    "$@"
  )
  fx-wait-ignoring-signals "fx-build" "${PREBUILT_PYTHON3}" "${main_build_script}" "${args[@]}"
}

# Run a Ninja build command after setting up the environment and prepending
# optional wrappers for RBE and profiling to it.
# Arguments:
#    print_full_cmd   if true, prints the full command line before
#                     executing it
#    ninja command    the ninja command itself.
function fx-run-ninja {
  fx-run-build-command "$1" "ninja" "${@:2}"
}

# Run a Bazel build command after setting up the environment and prepending
# optional wrappers for RBE and profiling to it.
# Arguments:
#    print_full_cmd   if true, prints the full command line before
#                     executing it
#    bazel command    the bazel command itself.
function fx-run-bazel {
  fx-run-build-command "$1" "bazel" "${@:2}"
}

# Run a fint build command after setting up the environment, prepending
# optional wrappers for RBE and profiling to it, and generating a
# context textproto with the right concurrency.
# Arguments:
#    print_full_cmd   if true, prints the full command line before
#                     executing it
#    fint command     the fint command itself.
function fx-run-fint {
  fx-run-build-command "$1" "fint" "${@:2}"
}

function fx-get-image {
  fx-command-run list-build-artifacts --name "$1" --type "$2" --expect-one images
}

function fx-get-zbi {
  fx-get-image "$1" zbi
}

function fx-get-qemu-kernel {
  fx-get-image qemu-kernel kernel
}

function fx-zbi {
  "${FUCHSIA_BUILD_DIR}/$(fx-command-run list-build-artifacts --name zbi --expect-one tools)" --compressed="$FUCHSIA_ZBI_COMPRESSION" "$@"
}

function fx-zbi-default-compression {
  "${FUCHSIA_BUILD_DIR}/$(fx-command-run list-build-artifacts --name zbi --expect-one tools)" "$@"
}

# Failsafe check for global TUI functionality, defaults to 'text' in case of
# errors. Call with e.g. $(fx-get-ui-mode "fx-use") to check for command
# specific overrides. By convention use snake-case to identify commands in
# overrides, e.g. "fx-use" or "ffx-target-list".
# Echos "tui" or "text" for the two modes, returns 0 on error-free invocation
# and returns 1 if any of the called commands have errors.
function fx-get-ui-mode() {
  local command="$1"
  local override_mode
  local mode

  mode="text" # default to text
  # We are NOT using fx-command-run to avoid having to build `ffx` -
  # one instance of this call chooses which out dir to use with `fx use`
  local ffx_binary="${FUCHSIA_BUILD_DIR}/host-tools/ffx"
  if [[ ! -x "${ffx_binary}" ]]; then
    fx-warn "ffx not found in build directory. It is needed to check \`ffx.ui.mode\`. Defaulting to 'text'"
    echo "text"
    return 1
  fi
  # check for the ffx.ui.mode
  tui_setting_raw="$("${ffx_binary}" config get ffx.ui.mode 2>/dev/null)"
  exit_status=$?
  if [[ "$exit_status" -ne 0 ]]; then
    fx-warn "error running ffx config get ffx.ui.mode, defaulting to 'text'"
    echo "text"
    return 1
  elif [[ -n "$tui_setting_raw" ]]; then
    tui_setting="${tui_setting_raw#\"}" # Remove leading " if it exists
    tui_setting="${tui_setting%\"}" # Remove trailing " if it exists
    mode="${tui_setting}"
  fi
  # there can be command specific overrides, check for those
  if [[ -n $command ]]; then
    overrides="$("${ffx_binary}" config get ffx.ui.overrides 2>/dev/null)"
    exit_status=$?
    if [[ "$exit_status" -ne 0 && "$exit_status" -ne 2 ]]; then # 2 is Value not found, which not an error
        fx-warn "Error calling ffx config get ffx.ui.overrides, $mode"
        echo "$mode"
        return 1
    else
      override_mode="$(fx jq --arg cmd "$command" -r '.[$cmd] //empty' <<< "$overrides")"
      exit_status=$?
      if [[ "$exit_status" -ne 0 ]]; then
        fx-warn "Error parsing overrides in fx-get-ui-mode, using $mode"
        echo "$mode"
        return 1
      elif [[ -n "$override_mode" ]]; then
          mode="$override_mode"
      fi
    fi
  fi
  # check the mode set is in the right format (or default to text)
  case "$mode" in
        text|tui)
          echo "$mode"
          ;;
        *)
          fx-warn "Warning: Invalid mode '$mode' detected for ffx.ui.mode, but 'tui' or 'text' expected: defaulting to 'text'\n\t (Change the setting with e.g. 'ffx config set ffx.ui.mode tui' )" >&2
          echo "text"
          ;;
  esac
  return 0
}

# Gum - wrapped functionality for choosing amongst options
function fx-choose-tui {
    fx-command-run gum choose "$@"
}

# Gum - wrapped functionality for choosing amongst options
function fx-filter-tui {
    fx-command-run gum filter "$@"
}

# Writes a ResultStore config properties file atomically with embedded documentation.
# Usage: fx-resultstore-write-config <file_path> <value>
function fx-resultstore-write-config {
  local path="$1"
  local val="$2"
  local enabled=0
  if [[ "$val" != "none" ]]; then
    enabled=1
  fi

  local -r tempfile="${path}.tmp"
  mkdir -p "$(dirname "${path}")"

  cat > "${tempfile}" <<EOF
# Autogenerated config file for ResultStore.
# Valid values for 'resultstore':
#   all:   Enable uploading for both Ninja and Bazel builds.
#   ninja: Enable uploading only for Ninja builds.
#   bazel: Enable uploading only for Bazel builds.
#   none:  Disable all ResultStore uploading.
#
RESULTSTORE_ENABLED=${enabled}
resultstore=${val}
EOF
  # Only rewrite the config file if content has changed
  if ! cmp --silent "${tempfile}" "${path}"
  then
    mv -f "${tempfile}" "${path}"
    echo "Writing ResultStore configuration to ${path}"
  else
    rm -f "${tempfile}"
  fi
}

fi  # -z "${lib_vars_sh_sourced:-}"
