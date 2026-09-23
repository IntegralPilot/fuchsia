# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

function mfcd {
  local -r MULTIFUCHSIA_ROOT={{placeholder}}
  local -r MOUNTPOINT="$($MULTIFUCHSIA_ROOT/multifuchsia config_get_mountpoint)"
  if [ $# -eq 0 ]; then
    cd "$MULTIFUCHSIA_ROOT"
    return 0
  elif [ $# -eq 1 ]; then
    local -r dir="$1"
    if [[ "$(realpath "$(pwd)")"/ == "$MOUNTPOINT"/* ]]; then
      # just in case this shell only thing keeping $MOUNTPOINT busy, cd out of it
      cd "$MOUNTPOINT/.."
    fi
    "$MULTIFUCHSIA_ROOT/multifuchsia" mount "$dir" || return 1
    cd "$MOUNTPOINT"
  else
    echo "Too many arguments" >&2
    return 1
  fi
}
function _mfcd {
  local -r MULTIFUCHSIA_ROOT={{placeholder}}
  COMPREPLY=()
  local -r cur="${COMP_WORDS[COMP_CWORD]}"
  COMPREPLY=( $(cd "$MULTIFUCHSIA_ROOT"/checkouts/ && compgen -d "${cur}") )
  return 0
}
complete -o nospace -F _mfcd mfcd
function mfenter {
  local -r MULTIFUCHSIA_ROOT={{placeholder}}
  if [ $# -eq 0 ]; then
    echo "error: missing argument: checkout name" >&2
    return 1
  elif [ $# -eq 1 ]; then
    local -r dir="$1"
    "$MULTIFUCHSIA_ROOT/multifuchsia" enter "$MULTIFUCHSIA_ROOT/checkouts/$dir"
  else
    echo "Too many arguments" >&2
    return 1
  fi
}
function _mfenter {
  local -r MULTIFUCHSIA_ROOT={{placeholder}}
  COMPREPLY=()
  local -r cur="${COMP_WORDS[COMP_CWORD]}"
  COMPREPLY=( $(cd "$MULTIFUCHSIA_ROOT"/checkouts/ && compgen -d "${cur}") )
  return 0
}
complete -o nospace -F _mfenter mfenter
