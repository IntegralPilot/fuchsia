#!/bin/sh
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

usage() {
  cat <<EOF
$0 command...

This script filters a command as follows, before executing it:
  --local-only=* : retain the option argument as a command token
  --remote-only=* : removed

This script is suitable for 'rewrapper --local_wrapper'.
EOF
}

[[ "$#" > 1 ]] || {
  usage
  exit 0
}

# /usr/bin/env, in case command starts with an environment variable.
cmd=( /usr/bin/env )

prev_opt=""
for opt
do
  # handle --option arg
  if [[ -n "$prev_opt" ]]
  then
    case "$prev_opt" in
      keep_next) cmd+=( "$opt" );;
      drop_next) ;;
    esac
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
    --local-only=*) cmd+=( "$optarg" ) ;;
    --local-only) prev_opt=keep_next ;;
    --remote-only=*) ;;  # drop this token
    --remote-only) prev_opt=drop_next ;;
    *) cmd+=( "$opt" ) ;;
  esac
  shift
done

[[ -z "$prev_opt" ]] || {
  echo "Error: missing expected argument after last option"
  exit 1
}

exec "${cmd[@]}"
