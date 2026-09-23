#!/usr/bin/env bash

# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

function usage() {
  cat <<EOF
$0 [options] -- <rustc> [rustc-arguments...]

Options:
  --help | -h : print help and exit
  --timeout DURATION: how long to wait before killing rustc. the duration is
    passed through to timeout(1), see that man page for details. zero means
    no timeout.
  --check-ice: search the output for internal compiler error messages

EOF
}

duration=0
check_ice=0

# Extract options before --
prev_opt=
for opt do
  # handle --option arg
  if test -n "$prev_opt"; then
    eval "$prev_opt"=\$opt
    prev_opt=
    shift
    continue
  fi
  # Extract optarg from --opt=optarg
  case "$opt" in
    -*=*) optarg="${opt#*=}" ;;  # remove-prefix, shortest-match
  esac
  case "$opt" in
    --help|-h) usage ; exit ;;
    --timeout) prev_opt=duration ;;
    --timeout=*) duration="$optarg" ;;
    --check-ice) check_ice=1 ;;
    --) shift; break ;; # Everything else is part of the rustc command
  esac
  shift
done
test -z "$prev_opt" || { echo "Option is missing argument to set $prev_opt." ; exit 1;}

ice_err="internal compiler error"
ice_msg="\n\n\e[1;3;41mIt looks like you've encountered an INTERNAL COMPILER\n"
ice_msg+="ERROR (ICE for short). rustc's incremental compilation feature is a\n"
ice_msg+="common cause of ICEs. If you have 'rust_incremental = \"foo\"' in your\n"
ice_msg+="args.gn, try running 'rm out/default/foo' and re-running the build.\n"
ice_msg+="If after clearing your incremental dir you still get an ICE, file\n"
ice_msg+="a bug against Language Platforms > Rust.\e[m\n"

command=(env "$@")
if [[ $duration != 0 ]]; then
  command=(timeout "${duration}s" "${command[@]}")
fi

if [[ $check_ice == 1 ]]; then
  "${command[@]}" 2> >(tee >&2 \
    >(grep "$ice_err" > /dev/null && echo -e "$ice_msg" >&2))
else
  "${command[@]}"
fi
result="$?"

timeout_msg="\n\nERROR: The rust compiler timed out after $duration seconds\n"
[[ $result == 124 ]] && echo -e "$timeout_msg"

exit $result
