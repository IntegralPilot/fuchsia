#!/bin/bash
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Check if required environment variables are set
if [ -z "${FUCHSIA_SERIAL_UNIX_SOCKET}" ]; then
  echo "Error: FUCHSIA_SERIAL_UNIX_SOCKET environment variable is not set."
  echo "Please set it to the path of your serial UNIX socket."
  exit 1
fi

if [ -z "${FUCHSIA_SERIAL_LOG_FILE}" ]; then
  echo "Error: FUCHSIA_SERIAL_LOG_FILE environment variable is not set."
  echo "Please set it to the path of your serial log file."
  exit 1
fi

if [ -z "$1" ]; then
  echo "Usage: $0 \"<command>\""
  exit 1
fi

COMMAND="$1"

# Start tailing the log file in the background
# '-n 0' ensures we don't get older output
tail -n 0 -f "${FUCHSIA_SERIAL_LOG_FILE}" &
TAIL_PID=$!

# Give tail a brief moment to start up so we don't miss any output
sleep 0.2

# Echo the command into the unix socket
# Note: no need to add '\n' because echo adds it naturally, and socat passes it through
echo "${COMMAND}" | socat - "UNIX-CONNECT:${FUCHSIA_SERIAL_UNIX_SOCKET}"

# Intelligently wait for the output to stop.
# We will monitor the log file size. If it stops growing for 2 seconds,
# we assume the command has finished outputting.
LAST_SIZE=-1
IDLE_CHECKS=0
MAX_IDLE_CHECKS=4 # 4 * 0.5s = 2.0s

while true; do
  sleep 0.5
  CURRENT_SIZE=$(wc -c < "${FUCHSIA_SERIAL_LOG_FILE}")

  if [ "${CURRENT_SIZE}" -eq "${LAST_SIZE}" ]; then
    IDLE_CHECKS=$((IDLE_CHECKS + 1))
    if [ "${IDLE_CHECKS}" -ge "${MAX_IDLE_CHECKS}" ]; then
      break # No new output for 2 seconds
    fi
  else
    LAST_SIZE="${CURRENT_SIZE}"
    IDLE_CHECKS=0
  fi
done

# Kill the tail process once output is assumed to be complete
kill "${TAIL_PID}" >/dev/null 2>&1
wait "${TAIL_PID}" 2>/dev/null

exit 0
