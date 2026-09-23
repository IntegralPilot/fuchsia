#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script is to be sourced by other scripts that live in
# the same dir as this one, and should not executed by itself.
# Since this script is to be sourced at the beginning,
# it does not assume or require any other variables to be predefined.

# By sourcing this script, the following symbols are defined:
#   default_project_root (variable)
#   normalize_path (function)
#   msg (function)
#   relpath (function)
#   timetrace (function)
#   HOST_PLATFORM (variable): prebuilt subdir for the host platform
#   python (variable): path to prebuilt python interpreter for the host

# This script caches some computed values in environment variables
# to avoid repetitive work in related scripts.
# Environment variables used and exported by this script:
#   _FUCHSIA_RBE_CACHE_VAR_relpath_uses
#   _FUCHSIA_RBE_CACHE_VAR_host_os
#   _FUCHSIA_RBE_CACHE_VAR_host_arch

readonly _invoking_script="$0"  # This is the name of the invoking script, not this one.
[[ "$_invoking_script" == */* ]] || {
  echo >&2 "$0 must be invoked with a path prefix."
  exit 2
}
readonly _invoking_script_dir="${_invoking_script%/*}"  # dirname
readonly _invoking_script_basename="${_invoking_script##*/}"  # basename

function msg() {
  echo "[$_invoking_script_basename]: $@"
}

function timetrace() {
  # $EPOCHREALTIME has microsecond resolution and is available in bash 5.0+
  # This is preferred, as it incurs the least measurement overhead.
  echo "[@$EPOCHREALTIME] [$_invoking_script_basename]: $*" >&2
}

# Normalize path: return an absolute path without any .. in the middle.
# Following-symlinks is optional.
if which realpath 2>&1 > /dev/null; then
  function normalize_path() {
    realpath "$1"
  }
elif which readlink 2>&1 > /dev/null; then
  function normalize_path() {
    readlink -f "$1"
  }
else
  msg "Error: Unable to normalize paths."
  exit 1
fi

# This should point to $FUCHSIA_DIR for the Fuchsia project.
# ../../ because this script lives in build/rbe.
# The value is an absolute path.
readonly default_project_root="$(normalize_path "$_invoking_script_dir"/../..)"
test -n "$default_project_root" || {
  msg "Error: Unable to infer project root."
  exit 1
}

# OS/Arch detection logic can also be found in 'tools/devshell/lib/platform.sh'
[[ -n "${_FUCHSIA_RBE_CACHE_VAR_host_os+x}" ]] || {
  # This is cached to avoid repeating calls to uname.
  detected_os="$(uname -s)"
  case "$detected_os" in
    Darwin) export _FUCHSIA_RBE_CACHE_VAR_host_os="mac" ;;
    Linux) export _FUCHSIA_RBE_CACHE_VAR_host_os="linux" ;;
    *) echo >&2 "Unknown operating system: $detected_os" ; exit 1 ;;
  esac
}

[[ -n "${_FUCHSIA_RBE_CACHE_VAR_host_arch+x}" ]] || {
  # This is cached to avoid repeating calls to uname.
  detected_arch="$(uname -m)"
  case "$detected_arch" in
    x86_64) export _FUCHSIA_RBE_CACHE_VAR_host_arch="x64" ;;
    arm64) export _FUCHSIA_RBE_CACHE_VAR_host_arch="arm64" ;;
    *) echo >&2 "Unknown machine architecture: $detected_arch" ; exit 1 ;;
  esac
}

# This points to the host platform subdir of our prebuilt tools.
readonly HOST_PLATFORM="${_FUCHSIA_RBE_CACHE_VAR_host_os}-${_FUCHSIA_RBE_CACHE_VAR_host_arch}"

# Point to our prebuilt python3.
readonly python="$default_project_root"/prebuilt/third_party/python3/"$HOST_PLATFORM"/bin/python3

function _check_realpath_works_for_relative_paths() {
  if which realpath
  then
    # GNU coreutils' realpath is different from BSD utils' realpath.
    # Test if it is usable for calculating relative paths.
    realpath -s --relative-to=.. "$_invoking_script" 2>&1 > /dev/null || return 1
    return 0  # success
  else
    # realpath doesn't even exist
    return 1
  fi 2>&1 > /dev/null
}

[[ -n "${_FUCHSIA_RBE_CACHE_VAR_relpath_uses+x}" ]] || {
  # realpath doesn't ship with Mac OS X (provided by coreutils package).
  # We only want it for calculating relative paths.
  # Work around this using Python as needed.
  if _check_realpath_works_for_relative_paths
  then
    export _FUCHSIA_RBE_CACHE_VAR_relpath_uses=realpath
  else
    test -x "$python" || {
      msg "*** Python interpreter not found at expected location: $python."
      exit 1
    }
    export _FUCHSIA_RBE_CACHE_VAR_relpath_uses="$python"
  fi
}

# By this point, _FUCHSIA_RBE_CACHE_VAR_relpath_uses is definitely set.
case "$_FUCHSIA_RBE_CACHE_VAR_relpath_uses" in
  realpath)
    function relpath() {
      local -r from="$1"
      local -r to="$2"
      # Preserve symlinks.
      realpath -s --relative-to="$from" "$to"
    }
    ;;
  *python*)
    function relpath() {
      local -r from="$1"
      local -r to="$2"
      local -r _python="$_FUCHSIA_RBE_CACHE_VAR_relpath_uses"
      "$_python" -c "import os; print(os.path.relpath('$to', start='$from'))"
    }
    ;;
esac

