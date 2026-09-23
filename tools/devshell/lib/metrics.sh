#!/bin/bash
# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Common methods for metrics collection.
#
# Note: For non-shell programs, use metrics_custom_report.sh instead.
#
# Report events to the metrics collector from `fx`, the `fx metrics`, and the
# rare subcommand that has special metrics needs.
#
# How to use it: This file is sourced in //scripts/fx for tracking command
# execution and in //tools/devshell/metrics for managing the metrics collection
# settings. Developers of shell-based subcommands can source this file in their
# subcommand if they need custom event tracking, but only the method
# track-subcommand-custom-event can be used in this context.
#
# This script assumes that vars.sh has already been sourced, since it
# depends on FUCHSIA_DIR being defined correctly.

# Increase the metrics version by 1 when analytics is updated
_METRICS_VERSION="11"
_METRICS_ALLOWS_CUSTOM_REPORTING=( "test" )
# If args match the below, then track capture group 1
_METRICS_TRACK_REGEX=(
    "^run (fuchsia-pkg:\/\/[[:graph:]]*)"
    "^shell (run fuchsia-pkg:\/\/[[:graph:]]*)"
)
# We collect metrics when these operations happen without capturing all of
# their args.
_METRICS_TRACK_COMMAND_OPS=(
    "publish cache"
    "shell activity"
    "shell basename"
    "shell bssl"
    "shell bt-avdtp-tool"
    "shell bt-avrcp-controller"
    "shell bt-cli"
    "shell bt-hci-emulator"
    "shell bt-hci-tool"
    "shell bt-le-central"
    "shell bt-le-peripheral"
    "shell bt-pairing-tool"
    "shell bt-snoop-cli"
    "shell bugreport"
    "shell cal"
    "shell cat"
    "shell catapult_converter"
    "shell cksum"
    "shell cmp"
    "shell cols"
    "shell comm"
    "shell cowsay"
    "shell cp"
    "shell crashpad_database_util"
    "shell cs"
    "shell curl"
    "shell cut"
    "shell date"
    "shell dirname"
    "shell du"
    "shell echo"
    "shell ed"
    "shell env"
    "shell expand"
    "shell expr"
    "shell false"
    "shell far"
    "shell fdio_spawn_helper"
    "shell fdr"
    "shell find"
    "shell fold"
    "shell fuchsia_benchmarks"
    "shell gltf_export"
    "shell grep"
    "shell head"
    "shell hostname"
    "shell input"
    "shell iperf3"
    "shell iquery"
    "shell join"
    "shell limbo_client"
    "shell link"
    "shell locate"
    "shell log_listener"
    "shell ls"
    "shell md5sum"
    "shell mediasession_cli_tool"
    "shell mkdir"
    "shell mktemp"
    "shell mv"
    "shell net"
    "shell netdump"
    "shell nl"
    "shell od"
    "shell onet"
    "shell paste"
    "shell pathchk"
    "shell pkgctl"
    "shell present_view"
    "shell print_input"
    "shell printenv"
    "shell printf"
    "shell process_input_latency_trace"
    "shell pwd"
    "shell readlink"
    "shell rev"
    "shell rm"
    "shell rmdir"
    "shell run"
    "shell run_simplest_app_benchmark.sh"
    "shell run-test-suite"
    "shell scp"
    "shell screencap"
    "shell sed"
    "shell seq"
    "shell set_renderer_params"
    "shell sh"
    "shell sha1sum"
    "shell sha224sum"
    "shell sha256sum"
    "shell sha384sum"
    "shell sha512-224sum"
    "shell sha512-256sum"
    "shell sha512sum"
    "shell signal_generator"
    "shell sleep"
    "shell snapshot"
    "shell sort"
    "shell split"
    "shell sponge"
    "shell ssh"
    "shell ssh-keygen"
    "shell stash_ctl"
    "shell strings"
    "shell sync"
    "shell system-update-checker"
    "shell tail"
    "shell tar"
    "shell tee"
    "shell test"
    "shell tftp"
    "shell time"
    "shell touch"
    "shell tr"
    "shell trace"
    "shell true"
    "shell tsort"
    "shell tty"
    "shell uname"
    "shell unexpand"
    "shell uniq"
    "shell unlink"
    "shell update"
    "shell uudecode"
    "shell uuencode"
    "shell vim"
    "shell virtual_audio"
    "shell vol"
    "shell wav_recorder"
    "shell wc"
    "shell which"
    "shell whoami"
    "shell wlan"
    "shell xargs"
    "shell xinstall"
    "shell yes"
)

_METRICS_TRACK_UNKNOWN_OPS=( "shell" )

# These variables need to be global, but readonly (or declare -r) declares new
# variables as local when they are source'd inside a function.
# "declare -g -r" is the right way to handle it, but it is not supported in
# old versions of Bash, particularly in the one in MacOS. The alternative is to
# make them global first via the assignments above and marking they readonly
# later.
readonly _METRICS_VERSION _METRICS_ALLOWS_CUSTOM_REPORTING _METRICS_TRACK_REGEX _METRICS_TRACK_COMMAND_OPS _METRICS_TRACK_UNKNOWN_OPS

# To properly enable unit testing, METRICS_CONFIG is not read-only
METRICS_CONFIG="${FUCHSIA_DIR}/.fx/config/metrics"
# Old location of this file.  Will move to new location automatically.
readonly METRICS_CONFIG_OLD="${FUCHSIA_DIR}/.fx-metrics-config"

_METRICS_DEBUG=0
_METRICS_DEBUG_LOG_FILE=""
_METRICS_USE_VALIDATION_SERVER=0

INIT_WARNING=$'Please opt in or out of fx metrics collection.\n'
INIT_WARNING+=$'You will receive this warning until an option is selected.\n'
INIT_WARNING+=$'To check what data we collect, run `fx metrics`\n'
INIT_WARNING+=$'To opt in or out, run `fx metrics <enable|disable>\n'

INIT_WARNING_INTERNAL=$(
cat <<'EOF'
Enable enhanced analytics to help us improve Fuchsia tools!

You are identified as a Googler since your hostname ends with corp.google.com
or c.googlers.com. To better understand how Fuchsia tools are used, and help
improve these tools and your workflow, Google already has an option, as you
know, to collect basic, very redacted, analytics listed in
https://fuchsia.dev/fuchsia-src/contribute/governance/policy/analytics_collected_fuchsia_tools.
As a Googler, you can help us even more by opting in to enhanced analytics:

ffx config analytics enable-enhanced

You may need to first run `fx build` to get the latest ffx if the above
does not work.

Enabling enhanced analytics may collect the following additional information,
in accordance with Google's employee privacy policy
(go/employee-privacy-policy):

• Full command line arguments
• Product name (e.g. core, minimal, etc.)
• Board type (e.g. vim3, x64, etc.)
• Environment variables that affect behavior of the tools
• User/Host/Target environment (e.g. tool/system versions, connection type
  such as Network/USB/Remote, etc.)
• Full stacktrace
• Logs
• Content of configuration files (e.g. args.gn)

Before any data is sent, we will replace the value of $USER with the literal
string "$USER" and also redact $HOSTNAME in a similar way.

To collect basic analytics only, enter
  ffx config analytics enable
If you want to disable all analytics, enter
  ffx config analytics disable
To display the current setting and what is collected, type
  ffx config analytics show

You will continue to receive this notice until you select an option.

See Google's employee privacy policy:
go/employee-privacy-policy
EOF
)

# Each Analytics batch call can send at most this many hits.
declare -r BATCH_SIZE=25
# Keep track of how many hits have accumulated.
hit_count=0
# Holds events for the current batch
events=()

function __is_in {
  local v="$1"
  shift
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "$v" ]]; then
      return 0
    fi
    shift
  done
  return 1
}

function __is_in_regex {
  local v="$1"
  shift
  while [[ $# -gt 0 ]]; do
    if [[ $v =~ $1 ]]; then
      return 0
    fi
    shift
  done
  return 1
}

function _get-metrics-config-dir {
  local base_dir
  case "${HOST_OS}" in
    linux)
      base_dir="${XDG_DATA_HOME}"
      if [[ -z "${base_dir}" ]]; then
        base_dir="${HOME}/.local/share"
      fi
      ;;
    mac)
      base_dir="${HOME}/Library/Application Support"
      ;;
  esac
  echo "${base_dir}/Fuchsia/metrics"
}

function _read-other-tools-analytics-uuid {
  local base_dir="$(_get-metrics-config-dir)"
  echo "$(cat "${base_dir}/uuid" 2>/dev/null)"
}

function _write-other-tools-analytics-uuid {
  local base_dir="$(_get-metrics-config-dir)"
  mkdir -p "${base_dir}"
  echo "$1" >"${base_dir}/uuid"
}

function metrics-is-internal-user {
  # Due to old bash versions on mac, we don't support internal analytics on mac.
  [[ "$HOSTNAME" =~ (".c.googlers.com"$)|(".corp.google.com"$)  && "${HOST_OS}" != "mac" ]]
}

function metrics-sanitize-string {
  local sanitized="${1//$FUCHSIA_DIR/\$FUCHSIA_DIR}"
  sanitized="${sanitized//$USER/\$USER}"
  sanitized="${sanitized//$HOSTNAME/\$HOSTNAME}"
  echo "$sanitized"
}

function metrics-read-config-external {
  METRICS_UUID=""
  METRICS_ENABLED=0
  _METRICS_DEBUG_LOG_FILE=""
  if [[ ! -f "${METRICS_CONFIG}" ]]; then
    return 1
  fi
  source "${METRICS_CONFIG}"
  if [[ $METRICS_ENABLED == 1 && -z "$METRICS_UUID" ]]; then
    METRICS_ENABLED=0
    return 1
  fi
  OTHER_TOOLS_ANALYTICS_UUID="$(_read-other-tools-analytics-uuid)"
  return 0
}

function metrics-read-config-internal {
  local base_dir="$(_get-metrics-config-dir)"
  local metrics_config_internal="${base_dir}/analytics-status-internal"
  METRICS_LEVEL=0

  # Non-migrated user
  if [[ ! -f "${metrics_config_internal}" ]]; then
    if [[ -f "${METRICS_CONFIG}" ]]; then
      metrics-read-config-external
      metrics-set-level-from-enabled
    fi
    return
  fi

  # Migrated user
  METRICS_UUID=""
  _METRICS_DEBUG_LOG_FILE=""
  if [[ -f "${METRICS_CONFIG}" ]]; then
    source "${METRICS_CONFIG}"
  fi

  METRICS_LEVEL="$(cat "${metrics_config_internal}" 2>/dev/null)"
  if [[ "$METRICS_LEVEL" -ne 1 && "$METRICS_LEVEL" -ne 2  ]]; then
    METRICS_LEVEL=0
  fi

  if [[ "${METRICS_LEVEL}" -gt 0 ]]; then
    OTHER_TOOLS_ANALYTICS_UUID="$(_read-other-tools-analytics-uuid)"
    if [[ -z "${METRICS_UUID}" ]]; then
      uuidgen_cmd=uuidgen
      if ! command -v "$uuidgen_cmd" >/dev/null 2>&1 ; then
        fx-error "Command '$uuidgen_cmd' cannot be found, please add it to your PATH."\
                "(On Ubuntu/Debian systems, try \`sudo apt install uuid-runtime\`.)"
        exit 1
      fi
      METRICS_UUID="$($uuidgen_cmd)"
      metrics-write-config-internal "${METRICS_UUID}" "${_METRICS_DEBUG_LOG_FILE}"
      return
    fi

    if [[ -n "${ENABLED}" ]]; then
      metrics-write-config-internal "${METRICS_UUID}" "${_METRICS_DEBUG_LOG_FILE}"
    fi
  fi

}



function metrics-read-config {
  if metrics-is-internal-user; then
    metrics-read-config-internal
  else
    metrics-read-config-external
    # External analytics relies on this exit code to correctly disable metrics.
    local exit_code=$?
    metrics-set-level-from-enabled
    return $exit_code
  fi
}

function metrics-write-config {
  local enabled=$1
  if [[ "$enabled" -eq "1" ]]; then
    local uuid="$2"
    if [[ $# -gt 2 ]]; then
      local debug_logfile="$3"
    fi
  fi
  local -r tempfile="$(mktemp)"

  # Exit trap to clean up temp file
  trap "[[ -f \"${tempfile}\" ]] && rm -f \"${tempfile}\"" EXIT

  {
    echo "# Autogenerated config file for fx metrics. Run 'fx help metrics' for more information."
    echo "METRICS_ENABLED=${enabled}"
    echo "METRICS_UUID=\"${uuid}\""
    if [[ -n "${debug_logfile}" ]]; then
      echo "_METRICS_DEBUG_LOG_FILE=${debug_logfile}"
    fi
  } >> "${tempfile}"
  # Only rewrite the config file if content has changed
  if ! cmp --silent "${tempfile}" "${METRICS_CONFIG}" ; then
    mv -f "${tempfile}" "${METRICS_CONFIG}"
  fi
}

function metrics-write-config-internal {
  local uuid="$1"
  if [[ $# -gt 1 ]]; then
    local debug_logfile="$2"
  fi

  local -r tempfile="$(mktemp)"

  # Exit trap to clean up temp file
  trap "[[ -f \"${tempfile}\" ]] && rm -f \"${tempfile}\"" EXIT

  {
    echo "# Autogenerated config file for fx metrics."
    echo "METRICS_UUID=\"${uuid}\""
    if [[ -n "${debug_logfile}" ]]; then
      echo "_METRICS_DEBUG_LOG_FILE=${debug_logfile}"
    fi
  } >> "${tempfile}"
  # Only rewrite the config file if content has changed
  if ! cmp --silent "${tempfile}" "${METRICS_CONFIG}" ; then
    mv -f "${tempfile}" "${METRICS_CONFIG}"
  fi
}

function metrics-set-level-from-enabled {
  if [[ "${METRICS_ENABLED}" -eq 1 ]]; then
      METRICS_LEVEL=1
    else
      METRICS_LEVEL=0
  fi
}

function metrics-read-and-validate-external {
  local hide_init_warning=$1
  if ! metrics-read-config-external; then
    if [[ "${hide_init_warning}" -ne 1 ]]; then
      fx-warn "${INIT_WARNING}"
    fi
  fi
  metrics-set-level-from-enabled
}

function metrics-migrate-internal-users {
  local base_dir="$(_get-metrics-config-dir)"
  local metrics_config_internal="${base_dir}/analytics-status-internal"
  local hide_init_warning="$1"

  if [[ ! -f "${metrics_config_internal}" ]]; then
    local metrics_config_external="${base_dir}/analytics-status"
    if [[ ! -f "${metrics_config_external}" ]]; then
      # New user
      if [[ "${hide_init_warning}" -ne 1 ]]; then
        fx-warn "$INIT_WARNING_INTERNAL"
      fi
      return
    fi

    # Existing user
    METRICS_LEVEL_EXISTING=$(cat "${metrics_config_external}" 2>/dev/null)
    if [[ "${METRICS_LEVEL_EXISTING}" -eq 1 ]]; then
      # Existing user opted-in
      if [[ "${hide_init_warning}" -ne 1 ]]; then
        fx-warn "$INIT_WARNING_INTERNAL"
      fi
    else
      # Existing user opted-out
      mkdir -p "${base_dir}"
      echo 0 >"${metrics_config_internal}"
    fi
    return
  fi
}

function metrics-read-and-validate {
  if [[ -f "${METRICS_CONFIG_OLD}" ]]; then
    fx-info "Moving ${METRICS_CONFIG_OLD} to new location ${METRICS_CONFIG}.  No further action is needed."
    mv "${METRICS_CONFIG_OLD}" "${METRICS_CONFIG}"
  fi
  if metrics-is-internal-user; then
    metrics-migrate-internal-users "$1"
    metrics-read-config-internal
  else
    metrics-read-and-validate-external "$1"
  fi
}

function metrics-set-debug-logfile {
  _METRICS_DEBUG_LOG_FILE="$1"
  return 0
}

function metrics-get-debug-logfile {
  if [[ -n "$_METRICS_DEBUG_LOG_FILE" ]]; then
    echo "$_METRICS_DEBUG_LOG_FILE"
  fi
}

function metrics-maybe-log {
  local filename="$(metrics-get-debug-logfile)"
  if [[ -n "$filename" ]]; then
    if [[ ! -f "$filename" && -w $(dirname "$filename") ]]; then
      touch "$filename"
    fi
    if [[ -w "$filename" ]]; then
      TIMESTAMP="$(date +%Y%m%d_%H%M%S)"
      echo -n "${TIMESTAMP}:" >> "$filename"
      for i in "$@"; do
        if [[ "$i" =~ ^"--" ]]; then
          continue # Skip switches.
        fi
        # Space before $i is intentional.
        echo -n " $i" >> "$filename"
      done
      # Add a newline at the end.
      echo >> "$filename"
    fi
  fi
}

# Arguments:
#   - the name of the fx subcommand
#   - event action
#   - (optional) event label
function track-subcommand-custom-event {
  exec 1>/dev/null
  exec 2>/dev/null
  local subcommand="$1"
  local event_action="$2"
  shift 2
  local event_label="$*"

  # Only allow custom arguments to subcommands defined in # $_METRICS_ALLOWS_CUSTOM_REPORTING
  if ! __is_in "$subcommand" "${_METRICS_ALLOWS_CUSTOM_REPORTING[@]}"; then
    return 1
  fi

  # Limit to the first 100 characters
  # The Analytics API supports up to 500 bytes, but it is likely that
  # anything larger than 100 characters is an invalid execution and/or not
  # what we want to track.
  event_label=${event_label:0:100}

  metrics-read-config
  if [[ "${METRICS_LEVEL}" -eq 0 ]]; then
    return 0
  fi

  event_params=$(fx-command-run jq -c -n \
    --arg subcommand "${subcommand}" \
    --arg action "${event_action}" \
    --arg label "${event_label}" \
    '$ARGS.named')
  _add-to-analytics-batch "custom" "${event_params}"
  # Send any remaining hits.
  _send-analytics-batch
  return 0
}

# Arguments:
#   - the name of the fx subcommand
#   - args of the subcommand
function track-command-execution {
  exec 1>/dev/null
  exec 2>/dev/null
  local subcommand="$1"
  shift
  local args="$*"
  local subcommand_op;
  if [[ $# -gt 0 ]]; then
    local subcommand_arr=( $args )
    subcommand_op=${subcommand_arr[0]}
  fi

  metrics-read-config
  if [[ "${METRICS_LEVEL}" -eq 0 ]]; then
    return 0
  fi

  if [[ "$subcommand" == "set" ]]; then
    # Add separate fx_set hits for packages
    _process-fx-set-command "$@"
  fi

  if [[ "${METRICS_LEVEL}" -eq 2 ]]; then
    args="$(metrics-sanitize-string "${args}")"
  elif __is_in_regex "${subcommand} ${args}" "${_METRICS_TRACK_REGEX[@]}"; then
    args="${BASH_REMATCH[1]}"
  elif [ -n "${subcommand_op}" ]; then
    if __is_in "${subcommand} ${subcommand_op}" \
        "${_METRICS_TRACK_COMMAND_OPS[@]}"; then
      # Track specific subcommand arguments (instead of all of them)
      args="${subcommand_op}"
    elif __is_in "${subcommand}" "${_METRICS_TRACK_UNKNOWN_OPS[@]}"; then
      # We care about the fact there was a subcommand_op, but we haven't
      # explicitly opted it into metrics collection.
      args="\$unknown_subcommand"
    else
      args=""
    fi
  else
    # Track no arguments
    args=""
  fi

  # Limit to the first 500 characters of arguments.
  # The GA4 360 supports up to 500 characters for parameter values
  local args_truncated=0
  local args1="${args:0:500}"
  local args2="${args:500:500}"
  if [[ "${#args}" -gt 1000 ]]; then
    args_truncated=1
  fi

  local call_stack="${FUCHSIA_METRICS_CALL_STACK:0:500}"

  event_params=$(fx-command-run jq -c -n \
    --arg subcommand "${subcommand}" \
    --arg args "${args1}" \
    --arg args2 "${args2}" \
    --arg args_truncated "${args_truncated}" \
    --arg invoker "${FUCHSIA_FX_INVOKER}" \
    --arg call_stack "${call_stack}" \
    '$ARGS.named')

  _add-to-analytics-batch "invoke" "${event_params}"
  # Send any remaining hits.
  _send-analytics-batch
  return 0
}

# Arguments:
#   - args of `fx set`
function _process-fx-set-command {
  while [[ $# -ne 0 ]]; do
    case $1 in
      --with)
        shift # remove "--with"
        _add-fx-set-hit "fx-with" "$1"
        ;;
      --with-base)
        shift # remove "--with-base"
        _add-fx-set-hit "fx-with-base" "$1"
        ;;
      *)
        ;;
    esac
    shift
  done
}

# Arguments:
#   - category name, either "fx-with" or "fx-with-base"
#   - package(s) following "--with" or "--with-base" switch
function _add-fx-set-hit {
  category="$1"
  packages="$2"
  # Packages argument can be a comma-separated list.
  IFS=',' read -ra packages_parts <<< "$packages"
  for p in "${packages_parts[@]}"; do
    event_params=$(fx-command-run jq -c -n \
      --arg with_type "${category}" \
      --arg package_name "${p}" \
      '$ARGS.named')

    _add-to-analytics-batch "set" "${event_params}"
  done
}

# Arguments:
#   - time taken to complete (milliseconds)
#   - exit status
#   - the name of the fx subcommand
#   - args of the subcommand
function track-command-finished {
  exec 1>/dev/null
  exec 2>/dev/null
  local start_time="$1"
  local end_time="$2"
  local exit_status="$3"
  local subcommand="$4"
  local is_remote="$5"
  shift 5
  local args="$*"

  metrics-read-config
  if [[ "${METRICS_LEVEL}" -eq 0 ]]; then
    return 0
  fi

  # Only track arguments to the subcommands in $_METRICS_TRACK_ALL_ARGS
  if [[ "${METRICS_LEVEL}" -eq 2 ]]; then
    args="$(metrics-sanitize-string "${args}")"
  elif ! __is_in "$subcommand" "${_METRICS_TRACK_ALL_ARGS[@]}"; then
    args=""
  fi

  local args_truncated=0
  local args1="${args:0:500}"
  local args2="${args:500:500}"
  if [[ "${#args}" -gt 1000 ]]; then
    args_truncated=1
  fi

  local call_stack="${FUCHSIA_METRICS_CALL_STACK:0:500}"

  local timing=$(( (end_time - start_time)/1000 ))

  event_params=$(fx-command-run jq -c -n \
    --arg subcommand "${subcommand}" \
    --arg args "${args1}" \
    --arg args2 "${args2}" \
    --arg args_truncated "${args_truncated}" \
    --arg exit_status "${exit_status}" \
    --arg is_remote "${is_remote}" \
    --arg invoker "${FUCHSIA_FX_INVOKER}" \
    --argjson timing "${timing}" \
    --argjson start_time_micros "${start_time}" \
    --argjson end_time_micros "${end_time}" \
    --arg call_stack "${call_stack}" \
    '$ARGS.named')

  _add-to-analytics-batch "finish" "${event_params}"


  # Send any remaining hits.
  _send-analytics-batch
  return 0
}

# Arguments:
#   - feature name
#   - 0 for enabled, 1 for disabled
function track-feature-status {
  if [[ "${HOST_OS}" == "mac" ]]; then
    return
  fi

  exec 1>/dev/null
  exec 2>/dev/null

  metrics-read-config
  if [[ "${METRICS_LEVEL}" -eq 0 ]]; then
    return 0
  fi

  local feature=$1
  local is_disabled=$2
  local status

  if [[ "${is_disabled}" -eq 0 ]]; then
      status="enabled"
  else
      status="disabled"
  fi

  event_params=$(fx-command-run jq -c -n \
    --arg feature "${feature}" \
    --arg status "${status}" \
    '$ARGS.named')

  _add-to-analytics-batch "feature" "${event_params}"
}

function track-build-event {
  exec 1>/dev/null
  exec 2>/dev/null
  local start_time="$1"
  local end_time="$2"
  local exit_status="$3"
  local ninja_switches="$4"
  local fuchsia_targets="$5"
  local build_dir="$6"
  local is_no_op="$7"
  local is_clean_build="$8"
  local quiet="$9"

  local args_json=""
  local env_flags=""
  local main_pb_label=""

  metrics-read-config
  if [[ "${METRICS_LEVEL}" -eq 0 ]]; then
    return 0
  fi

  if [[ "${METRICS_LEVEL}" -eq 2 ]]; then
    ninja_switches="$(metrics-sanitize-string "${ninja_switches}")"
    fuchsia_targets="$(metrics-sanitize-string "${fuchsia_targets}")"
    args_json=$(fx-command-run jq -c '{ b: .build_info_board, p: .build_info_product, r: .rbe_mode, c: .compilation_mode, o: .optimize, sv: .select_variant, tc: .target_cpu, ri: .rust_incremental }' "${build_dir}"/args.json)
    main_pb_label=$(fx-command-run jq -r '.main_pb_label // empty' "${build_dir}"/args.json)
    local legacy_flag_count=0
    if [[ -f "${build_dir}/args.json" ]]; then
      legacy_flag_count=$(fx-command-run jq -r 'keys[]' "${build_dir}/args.json" | grep -E '(_rbe_|restat_|_download_unstripped_binaries)' | wc -l)
    fi
  else
    ninja_switches=""
    fuchsia_targets=""
  fi

  ninja_switches="${ninja_switches:0:500}"
  fuchsia_targets="${fuchsia_targets:0:500}"

  local args_json1="${args_json:0:500}"
  local args_json2="${args_json:500:500}"

  if [[ "${FUCHSIA_FX_ITERATIVE}" -eq 1 ]]; then
    env_flags="${env_flags}+iterative"
  else
    env_flags="${env_flags}-iterative"
  fi

  if [[ "${FUCHSIA_FX_TEST_RUN}" -eq 1 ]]; then
    env_flags="${env_flags}+test"
  else
    env_flags="${env_flags}-test"
  fi

  if [[ "${FUCHSIA_FX_MULTI_RUN}" -eq 1 ]]; then
    env_flags="${env_flags}+multi"
  else
    env_flags="${env_flags}-multi"
  fi

  if [[ "${quiet}" == true ]]; then
    env_flags="${env_flags}+quiet"
  else
    env_flags="${env_flags}-quiet"
  fi

  local -i args_json_size=$(wc -c < "${build_dir}"/args.json)
  local -i target_count=0
  if [[ "${is_no_op}" -eq 0 ]]; then
    target_count=$("${PREBUILT_PYTHON3}" "$FUCHSIA_DIR/tools/devshell/contrib/lib/count-ninja-actions.py")
  fi

  event_params=$(fx-command-run jq -c -n \
    --arg args_json1 "${args_json1}" \
    --arg args_json2 "${args_json2}" \
    --arg main_pb_label "${main_pb_label}" \
    --arg ninja_switches "${ninja_switches}" \
    --arg fuchsia_targets "${fuchsia_targets}" \
    --arg exit_status "${exit_status}" \
    --argjson start_time_micros "${start_time}" \
    --argjson end_time_micros "${end_time}" \
    --argjson target_count "${target_count}" \
    --argjson is_clean_build "${is_clean_build}" \
    --arg env_flags "${env_flags}" \
    --argjson args_json_size "${args_json_size}" \
    --arg invoker "${FUCHSIA_FX_INVOKER}" \
    --argjson legacy_flag_count "${legacy_flag_count}" \
    '$ARGS.named')

  _add-to-analytics-batch "build" "${event_params}"


  # Send any remaining hits.
  _send-analytics-batch
  return 0
}

function track-test-event {
  exec 1>/dev/null
  exec 2>/dev/null
  local start_time="$1"
  local end_time="$2"
  local exit_status="$3"
  local was_aborted="$4"
  local stats_json="$5"
  local build_dir="$6"

  metrics-read-config
  if [[ "${METRICS_LEVEL}" -eq 0 ]] || ! metrics-is-internal-user; then
    return 0
  fi

  # Package event params in a single jq call directly reading args.json if present.
  # Extracts test_selection from stats_json and strips it from stats_json
  # before chunking across stats_json1 and stats_json2 to avoid duplication and
  # protect the 2000-char stats budget.
  local slurp_arg=()
  if [[ -f "${build_dir}/args.json" ]]; then
    slurp_arg=(--slurpfile args "${build_dir}/args.json")
  else
    slurp_arg=(--argjson args '[{}]')
  fi

  local event_params
  event_params=$(fx-command-run jq -c -n \
    "${slurp_arg[@]}" \
    --argjson start_time_micros "${start_time:-0}" \
    --argjson end_time_micros "${end_time:-0}" \
    --arg exit_status "${exit_status}" \
    --arg was_aborted "${was_aborted}" \
    --arg stats_json "${stats_json}" \
    'def parse_stats:
       (try fromjson catch null) as $s |
       if ($s | type) == "object" then
         { sel: ($s.sel // ""), stats: ($s | del(.sel) | tojson) }
       else
         { sel: "", stats: . }
       end;

     ($args[0] // {}) as $a |
     ($stats_json | parse_stats) as $s |
     {
       start_time_micros: $start_time_micros,
       end_time_micros: $end_time_micros,
       exit_status: $exit_status,
       was_aborted: $was_aborted,
       product: ($a.build_info_product // ""),
       board: ($a.build_info_board // ""),
       main_product_bundle: ($a.main_pb_label // ""),
       test_selection: ($s.sel | tostring | .[0:1000]),
       stats_json1: $s.stats[0:1000],
       stats_json2: $s.stats[1000:2000]
     }')

  _add-to-analytics-batch "test" "${event_params}"
  _send-analytics-batch
  return 0
}

# Add an analytics hit with the given args to the batch of hits. This will trigger
# sending a batch when the batch size limit is hit.
#
# Arguments:
#   - analytics arguments, e.g. "t=event" "ec=fx" etc.
function __add-to-analytics-batch {
  if [[ $# -eq 0 ]]; then
    return 0
  fi

  event_name=$1
  shift

  timestamp_micros=$("${PREBUILT_PYTHON3}" \
    -c 'import time; print(int(time.time() * 1000000))')
  if [[ $# -eq 0 ]]; then
    event=$(fx-command-run jq -c -n \
      --arg name "${event_name}" \
      --argjson timestamp_micros "${timestamp_micros}" \
      '$ARGS.named')
  else
    event=$(fx-command-run jq -c -n \
      --arg name "${event_name}" \
      --argjson params "$*" \
      --argjson timestamp_micros "${timestamp_micros}" \
      '$ARGS.named')
  fi
  events+=("$event")

  (( hit_count += 1 ))
  if ((hit_count == BATCH_SIZE)); then
    __send-analytics-batch
  fi
}

# The following list of allowed ninja persistent modes comes from
# https://fuchsia.googlesource.com/third_party/github.com/ninja-build/ninja/+/2005473679afc095025bd6db7d461590f8701e65/src/persistent_mode.cc#338
_ALLOWED_NINJA_PERSISTENT_MODE=( "" "0" "1" "on" "off" "client" "server" )
function _get_ninja_persistent_mode {
  if __is_in "${NINJA_PERSISTENT_MODE}" \
  "${_ALLOWED_NINJA_PERSISTENT_MODE[@]}"; then
    echo "${NINJA_PERSISTENT_MODE}"
  else
    echo unsupported
  fi
}

# Sends the current batch of hits to the Analytics server. As a side effect, clears
# the hit count and batch data.
function __send-analytics-batch {
  if [[ $hit_count -eq 0 ]]; then
    return 0
  fi

  local internal
  if metrics-is-internal-user; then
    internal=1
  else
    internal=0
  fi

  # Construct the measuremnt data
  local user_properties="{\"os\":{\"value\":\"$(uname -s)\"},\
  \"arch\":{\"value\":\"$(uname -m)\"},\
  \"shell\":{\"value\":\"$(_app_name)\"},\
  \"shell_version\":{\"value\":\"$(_app_version)\"},\
  \"kernel_release\":{\"value\":\"$(uname -rs)\"},\
  \"ninja_persistent\":{\"value\":\"$(_get_ninja_persistent_mode)\"},\
  \"other_uuid\":{\"value\":\"${OTHER_TOOLS_ANALYTICS_UUID}\"},\
  \"internal\":{\"value\":${internal}},\
  \"is_cog\":{\"value\":$(_is_cog)},\
  \"metrics_level\":{\"value\":${METRICS_LEVEL}},\
  \"metrics_version\":{\"value\":${_METRICS_VERSION}},\
  \"nproc\":{\"value\":$(_get_nproc)}\
  }"
  local events_json=$(fx-command-run jq -n -c '$ARGS.positional' \
    --jsonargs "${events[@]}")
  local measurement=$(fx-command-run jq -n -c \
    --arg client_id "${METRICS_UUID}" \
    --argjson events "${events_json}" \
    --argjson user_properties "${user_properties}" \
    '$ARGS.named')

  local url_path="mp/collect"
  local url_parameters=$(printf "\x61\x70\x69\x5f\x73\x65\x63\x72\x65\x74=xjiXbh8eSGiExMthvAXd6w&measurement_id=G-L2YHSDD8ZF")
  local result=""
  if [[ $_METRICS_DEBUG == 1 && $_METRICS_USE_VALIDATION_SERVER == 1 ]]; then
    url_path="debug/mp/collect"
  fi
  if [[ $_METRICS_DEBUG == 1 && $_METRICS_USE_VALIDATION_SERVER == 0 ]]; then
    # if testing and not using the validation server, always return 202
    result="202"
  elif [[ $_METRICS_DEBUG == 0 || $_METRICS_USE_VALIDATION_SERVER == 1 ]]; then
    result=$(curl -s -o /dev/null -w "%{http_code}" -L -X POST \
      -H "Content-Type: application/json" \
      --data-raw "${measurement}" \
      "https://www.google-analytics.com/${url_path}?${url_parameters}")
  fi
  metrics-maybe-log "${measurement}" "RESULT=${result}"

  # Clear batch.
  hit_count=0
  events=()
}

# Metrics/analytics are processed asynchronously by the following function.
function _metrics-service {
  # redirect stdout and stderr away from parent process so that parent
  # stdout and stderr can be safely closed. Otherwise, dart will
  # wait for stdout and stderr being closed, defeating the purpose of
  # async analytics processing.
  exec 1>/dev/null
  exec 2>/dev/null
  metrics-read-config
  while read -r -d $'\0' args; do
        local IFS=$'\n'
        read -r -d $'\0' -a request <<< "$args"
        if [[ ${#request[@]} -eq 0 ]]; then
          __send-analytics-batch
        else
          __add-to-analytics-batch "${request[@]}"
        fi
  done <&0
  __send-analytics-batch
}

function _send-signal-if-new-internal-user {
  local base_dir="$(_get-metrics-config-dir)"
  local metrics_config_internal="${base_dir}/analytics-status-internal"
  # For internal new users, send a new user signal with only UUID
  if metrics-is-internal-user && [[ ! -f "${metrics_config_internal}"  ]]; then
    local uuid="$(_read-other-tools-analytics-uuid)"
    if [[ -z "${uuid}" ]]; then
      uuid="$(uuidgen 2>/dev/null)"
      if [[ -z "${uuid}" ]]; then
        # Give up when there's no existing UUID and uuidgen does not exist/fails
        return
      fi
      _write-other-tools-analytics-uuid "${uuid}"
    fi

    # For a new user, metrics is not fully initialized, so we need to send the
    # analytics with more low level functions and special treatment.
    METRICS_UUID="new_user"
    OTHER_TOOLS_ANALYTICS_UUID="new_user"
    __add-to-analytics-batch "new_user" "{\"uuid\":\"${uuid}\"}"
    __send-analytics-batch
  fi
}

# Init metrics service by redirecting file descriptor 10 to the process
# substitution of metrics service.
function metrics-init {
  if [[ -z "${HOME}" ]]; then
    METRICS_LEVEL=0
    exec 10> /dev/null
    return
  fi

  local subcommand="$1"
  local args="$2"

  # We will not show user notices for metrics in the following cases:
  # - the subcommand is metrics
  # - the subcommand is ffx and there's no fuchsia.analytics.ffx_invoker=fx argument
  #   This is because when ffx has the --config fuchsia.analytics.ffx_invoker=fx argument
  #   it will not display the user notices, so we will need to display it. When ffx
  #   does not have the --config fuchsia.analytics.ffx_invoker=fx argument, we need
  #   to hide the user notices since ffx will display one.
  # - the subcommand with args is `ffx config analytics ...`. This is to make sure
  #   ffx config analytics <anything> will not show the user notices.
  # - the command is invoked by an AI agent (to avoid cluttering agent context).
  local hide_init_warning=0
  if [[ "$subcommand" == "metrics" \
        || ( "$subcommand" == "ffx" && ! "$args" =~ "fuchsia.analytics.ffx_invoker=fx" ) \
        || ( "$subcommand" == "ffx" && "$args" =~ "config analytics" ) \
  ]] || is_invoked_by_agent; then
    hide_init_warning=1
  fi
  metrics-read-and-validate "${hide_init_warning}"

  if [[ "${METRICS_LEVEL}" -eq 0 ]]; then
    exec 10> /dev/null
    # The following line will NOT send any analytics for external users
    # or explicitly opted-out users
    _send-signal-if-new-internal-user&
  else
    exec 10> >(_metrics-service)
  fi

  export FUCHSIA_METRICS_CALL_STACK="${FUCHSIA_METRICS_CALL_STACK:+${FUCHSIA_METRICS_CALL_STACK} }fx"
}

# Calls __add-to-analytics-batch asynchronously, via metrics service
function _add-to-analytics-batch {
  if [[ $# -eq 0 ]]; then
    return 0
  fi

  local IFS=$'\n'
  # printf will output \" as ". To avoid that, we use echo to send "$*"
  echo "$*" >&10
  printf "\0" >&10
}

# Calls __send-analytics-batch asynchronously, via metrics service
function _send-analytics-batch {
  printf "\0" >&10
}



function _os_data {
  if command -v uname >/dev/null 2>&1 ; then
    uname -rs
  else
    echo "Unknown"
  fi
}

function _app_name {
  if [[ -n "${BASH_VERSION}" ]]; then
    echo "bash"
  elif [[ -n "${ZSH_VERSION}" ]]; then
    echo "zsh"
  else
    echo "Unknown"
  fi
}

function _app_version {
  if [[ -n "${BASH_VERSION}" ]]; then
    echo "${BASH_VERSION}"
  elif [[ -n "${ZSH_VERSION}" ]]; then
    echo "${ZSH_VERSION}"
  else
    echo "Unknown"
  fi
}

function _get_nproc {
  if [[ "${METRICS_LEVEL}" -eq 2 ]]; then
    nproc
  else
    echo "-1"
  fi
}

function _is_cog {
  if [[ "${PWD}" =~ ^/google/cog/cloud/ || "${PWD}" =~ ^/google/cartfs/mount/ ]]; then
    echo 1
  else
    echo 0
  fi
}

# Args:
#   debug_log_file: string with a filename to save logs
#   use_validation_hit_server:
#          0 do not hit any Analytics server (for local tests)
#          1 use the Analytics validation Hit server (for integration tests)
#   config_file: string with a filename to save the config file. Defaults to
#          METRICS_CONFIG
function _enable_testing {
  _METRICS_DEBUG_LOG_FILE="$1"
  _METRICS_USE_VALIDATION_SERVER=$2
  if [[ $# -gt 2 ]]; then
    METRICS_CONFIG="$3"
  fi
  _METRICS_DEBUG=1
  METRICS_UUID="TEST"
}
