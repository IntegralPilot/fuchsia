#!/bin/bash
# Copyright 2017 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Portable function to truncate a file to a given length.
fx-truncate() {
  local -r file="$1"
  local -r size="$2"
  touch "${file}"

  if [[ -z "${size}" ]]; then
    echo >&2 "fx-truncate: size \"${size}\" not given"
    return 1
  fi

  case $(uname) in
    Darwin)
      mkfile -n "${size}" "${file}"
      ;;
    Linux)
      truncate -s "${size}" "${file}"
      ;;
    *)
      head -c "${size}" /dev/zero > "${file}"
      ;;
  esac
  return $?
}

# Converts human-readable numbers to an integer.  Uses power-of-two abbreviations (K == 1024, M ==
# 1048576, etc).
#   1314K   => 1345536
#   13M     => 13631488
#   1314000 => 1314000
convert_human_readable_num() {
  if [[ -z "${1}" ]]; then
    return 1
  fi
  conv=1
  val="$1"
  case ${1: -1} in
    "K")
      conv=1024
      val="${val%?}"
      ;;
    "M")
      conv=1048576
      val="${val%?}"
      ;;
    "G")
      conv=1073741824
      val="${val%?}"
      ;;
  esac
  echo "$(( ${val} * ${conv} ))"
}

# Compares $1 to $2 where either can be a human-readable number (e.g. 12K).
# Prints -1, 0, or 1.
compare_human_readable_nums() {
  left="$(convert_human_readable_num $1)"
  right="$(convert_human_readable_num $2)"
  if [[ -z "${left}" || -z "${right}" ]]; then
    return 1
  fi

  if [[ "$left" == "$right" ]]; then
    echo "0"
  else
    least=$(echo -e "$left\\n$right" | sort -n | head -n1)
    if [[ "$least" == "$left" ]]; then
      echo "-1"
    else
      echo "1"
    fi
  fi
}

_fx-extend-generic() {
  local -r file="$1"
  local -r size="$2"

  len="$(wc -c < ${file} | awk '{print $1}')"
  target="$(convert_human_readable_num ${size})"
  if [[ "$len" -lt "$target" ]]; then
    rem="$(( $target - $len ))"
    head -c "${rem}" /dev/zero >> "${file}"
  fi
}

# Like fx-truncate, but only ever extends a file (if the file would be truncated, this is a NOP).
fx-extend() {
  local -r file="$1"
  local -r size="$2"
  touch "${file}"

  if [[ -z "${size}" ]]; then
    echo >&2 "fx-extend: size \"${size}\" not given"
    return 1
  fi

  case $(uname) in
    Darwin)
      # mkfile isn't always available on macos.
      if [[ -n "$(which mkfile)" ]]; then
        len="$(stat -f %z ${file})"
        if [[ "$(compare_human_readable_nums ${len} ${size})" -lt "0" ]]; then
          mkfile -n "${size}" "${file}"
        fi
      else
        _fx-extend-generic "${file}" "${size}"
      fi
      ;;
    Linux)
      len="$(stat --format=%s ${file})"
      if [[ "$(compare_human_readable_nums ${len} ${size})" -lt "0" ]]; then
        truncate -s "${size}" "${file}"
      fi
      ;;
    *)
      _fx-extend-generic "${file}" "${size}"
      ;;
  esac
  return $?
}

fx-need-mtools() {
  for tool in "mmd" "mcopy"; do
    if ! which "${tool}" >&1 > /dev/null; then
      echo >&2 "Tool \"${tool}\" not found. You may need to install GNU mtools"
      return 1
    fi
  done
}