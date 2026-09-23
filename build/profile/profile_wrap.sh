#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Collect system-wide profile information using system_profiler.py
# for the duration of a wrapped command, logging outputs to file.

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"
readonly script_basename="${script##*/}"

verbose=0

function msg() {
  echo >&2 "[$script_basename] $*"
}

function ts_echo() {
  if [[ "$verbose" -ne 0 ]]; then
    echo "[@$EPOCHREALTIME] [$script_basename]: $*" >&2
  fi
}

function usage() {
  cat <<END
Usage: $script \
  --system-log system_logfile \
  [script_args] -- command...

options:
  -v | --verbose : run verbosely
END
}

system_logfile=
interval=1  # seconds
prev_opt=

for opt
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
    --system-log) prev_opt=system_logfile ;;
    --system-log=*) system_logfile="$optarg" ;;
    -n) prev_opt=interval ;;
    -n=*) interval="$optarg" ;;
    -v | --verbose) verbose=1 ;;
    --) shift ; break ;;
    *) echo "Unknown $0 option: $opt" ; usage ; exit 1 ;;
  esac
  shift
done

[[ -n "$system_logfile" ]] || {
  echo "--system-log is required."
  exit 1
}

# Everything else after '--' is the command to run.
cmd=("$@")

[[ "$#" > 0 ]] || { echo "Missing command to run (after --)." ; exit 1; }

shutdown_pids=()

if [[ -n "$system_logfile" ]]
then
  rm -f "$system_logfile"
  ts_echo "Starting background profile collection..."
  "${PREBUILT_PYTHON3:-python3}" -S -u "${script_dir}/system_profiler.py" \
    --interval "$interval" \
    --output "$system_logfile" \
    --pid "$$" \
    --metadata "FX_BUILD_UUID:${FX_BUILD_UUID:-}" &
  readonly system_profiler_pid=$!
  ts_echo "Background profiler started (PID: $system_profiler_pid)"
  shutdown_pids+=( "$system_profiler_pid" )
fi

# Terminate system_profiler when main command is complete (or interrupted).
function shutdown() {
  ts_echo "Shutting down system profiler..."
  if [[ "${#shutdown_pids[@]}" > 0 ]]
  then
    if [[ "${_interrupted:-0}" == "1" ]]; then
      msg "Stopping background profile collection..."
    fi
    kill "${shutdown_pids[@]}"
  fi
  ts_echo "System profiler teardown completed"
}
trap shutdown EXIT

# Wait for a command while ignoring signals to ensure the parent outlives the child.
# This prevents the shell from exiting prematurely and orphaning backgrounded
# subprocesses during a signal (like Ctrl-C).
#
# Because the command is run in the same process group, signals (like SIGINT)
# are broadcast by the TTY to both the shell and the child, so no manual
# signal forwarding is required here. Successive signals will continue to reach
# the child as long as it is alive.
function wait-ignoring-signals {
  local child_pid=""
  local sig_count=0

  # Acknowledge signals and forward them to the child.
  function _signal_acknowledgement_handler {
    local sig="$1"
    _interrupted=1
    sig_count=$((sig_count + 1))

    if [[ $sig_count -eq 1 ]]; then
      msg "Received ${sig}. Forwarding to child (PID: ${child_pid:-unknown}) and waiting for graceful shutdown..."
    else
      msg "Received ${sig} again (${sig_count}). Still waiting for cleanup..."
    fi

    if [[ -n "${child_pid}" ]]; then
       # Signal the child. Since we are likely in a separate process group
       # (due to wrappers above us or set -m below), we signal the group.
       kill -"${sig}" "-${child_pid}" 2>/dev/null || true
    fi
  }
  trap '_signal_acknowledgement_handler SIGINT' INT
  trap '_signal_acknowledgement_handler SIGTERM' TERM
  trap '_signal_acknowledgement_handler SIGHUP' HUP

  # Run the command in a subshell with default signal dispositions.
  # We use set -m to ensure the child is in its own process group,
  # making it easier to signal the entire sub-tree.
  set -m
  ( trap - INT TERM HUP ; exec "$@" ) &
  child_pid=$!
  set +m

  local status=0
  # Wait for the child process to finish, even if individual wait calls are
  # interrupted by trapped signals (such as SIGINT from Ctrl-C).
  while true; do
    # Try to wait for the child process.
    if wait "${child_pid}"; then
      # If wait succeeded (returned 0), the process exited successfully.
      status=0
      break
    fi
    # If wait returned non-zero, it could be because the child failed or
    # because the wait itself was interrupted by a signal.
    status=$?

    # Query the OS to see if the child process is still running.
    if ! kill -0 "${child_pid}" 2>/dev/null; then
      # If the child process is no longer running, wait one last time to reap
      # the zombie process and retrieve its actual exit status.
      wait "${child_pid}" && status=0 || status=$?
      break
    fi
    # If the process is still running, the signal merely interrupted our wait
    # call. Loop back and continue waiting.
  done

  trap - INT TERM HUP
  return "$status"
}

_interrupted=0
wait-ignoring-signals "${cmd[@]}"
exit "$?"
