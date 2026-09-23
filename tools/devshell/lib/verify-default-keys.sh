#!/bin/bash
# Copyright 2020 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

_SSH_IDENTITY='.ssh/fuchsia_ed25519'
_SSH_AUTH_KEYS='.ssh/fuchsia_authorized_keys'

readonly MIXED_KEY_MISMATCH_ERROR=66
readonly KEY_GENERATION_ERROR=67

function fail_with_mixed_mismatch {
  fx-error "
You have different SSH credentials for Fuchsia devices in your local machine
and on the remote server.

Local and remote ~/.ssh/fuchsia_ed25519 need to match.

Before you continue, update your local and remote source code.
This ensures that the host tools on both machines expect the SSH credentials
in a consistent location.

If you have been using local tools like 'fx flash-remote', the local
credentials are probably most recent and you can backup and delete
the remote keys on ${host}:

    ssh ${host} 'mkdir -p ~/.ssh/save && mv ~/.ssh/fuchsia_* ~/.ssh/save'

and then run this command again.

If you want to keep the remote credentials, backup and delete the local keys:

    mkdir -p ~/.ssh/save && mv ~/.ssh/fuchsia_* ~/.ssh/save

and then run this command again.

If you manage your SSH credentials manually, use the --no-check-ssh-keys flag to skip this check.
"
  exit ${MIXED_KEY_MISMATCH_ERROR}
}


# Check if there is a Fuchsia SSH key in HOME (used for the GN SDK)
function normalize_local_key {
  fx-info "Checking for ""${HOME}"/${_SSH_IDENTITY}""
  if [[ ! -f "${HOME}/${_SSH_IDENTITY}" ]]; then
    return 1
  fi
  return 0
}

# Check whether a Fuchsia SSH key is present on the remote host when
# the remote is not a Fuchsia tree.
function normalize_remote_key {
  # shellcheck disable=SC2029
  ssh ${ssh_base_args[@]+"${ssh_base_args[@]}"} "${host}" \
    "[[ -f \"$_SSH_IDENTITY\" ]]" || return ${_ERROR_NO_KEY}
}

function compare_remote_and_local {
  local -r remote_key="\${HOME}/${_SSH_IDENTITY}"
  local -r local_key="${HOME}/${_SSH_IDENTITY}"
  # shellcheck disable=SC2029
  ssh ${ssh_base_args[@]+"${ssh_base_args[@]}"} "${host}" "cat ${remote_key}" \
    | cmp -s "${local_key}" -
}

function copy_local_to_remote {
  scp ${ssh_base_args[@]+"${ssh_base_args[@]}"} -q -p "${HOME}/${_SSH_IDENTITY}" "${host}:.ssh/"
  # shellcheck disable=SC2029
  ssh ${ssh_base_args[@]+"${ssh_base_args[@]}"} "${host}" "cat >> ${_SSH_AUTH_KEYS}" < "${HOME}/${_SSH_AUTH_KEYS}"
}

function copy_remote_to_local {
  (
    # force subshell to limit scope of umask
    umask 077
    mkdir -p "$HOME/.ssh"
    scp ${ssh_base_args[@]+"${ssh_base_args[@]}"} -q -p "${host}:${_SSH_IDENTITY}" "$HOME/.ssh"
    umask 133
    # shellcheck disable=SC2029
    ssh ${ssh_base_args[@]+"${ssh_base_args[@]}"} "${host}" cat "${_SSH_AUTH_KEYS}" >> "${HOME}/${_SSH_AUTH_KEYS}"
  )
}

function is_local_a_tree {
  test -n "$local_fuchsia_dir"
}

function is_remote_a_tree {
  test -n "$remote_fuchsia_dir"
}

# Verify if the Fuchsia SSH keys in the current host and on a remote host
# are the same.
#
# If no default identity is available on one of the local or remote machine,
# this script copies that identity into place on the other machine. If
# mismatched identities exist, this script prints an actionable error
# message and returns a status code of 113 (_ERROR_MISMATCHED_KEYS).
#
# Callers should have an user-facing '--no-check-ssh-keys' flag that
# skips this method if the user prefers to manage SSH credentials manually.
#
# Args:
#   1:   local_fuchsia_dir (empty string if GN SDK)
#   2:   Remote host
#   3:   Remote local_fuchsia_dir (empty string if GN SDK)
#   4-*: Additional args for `ssh`

function verify_default_keys {
  local -r local_fuchsia_dir="$1"
  local -r host="$2"
  local -r remote_fuchsia_dir="$3"
  shift 3
  local temp_ssh_args=()
  while [[ $# -gt 0 ]]; do
    if [[ "$1" == "-S" ]]; then
      # "-S" doesn't work for scp, so we transform it to its equivalent -o ControlPath=...
      shift
      if [[ $# -gt 0 ]]; then
        temp_ssh_args+=("-o" "ControlPath=$1")
      fi
    else
      temp_ssh_args+=( $1 )
    fi
    shift
  done

  local -r ssh_base_args=( "${temp_ssh_args[@]+"${temp_ssh_args[@]}"}" )

  local has_local_key=false
  local has_remote_key=false

  local -r _ERROR_NO_KEY=112
  local -r _ERROR_MISMATCHED_KEYS=113

  if normalize_local_key; then
    has_local_key=true
  fi

  if normalize_remote_key; then
    has_remote_key=true
  fi


  if ! $has_local_key && ! $has_remote_key; then
      # if remote is not a Fuchsia tree, it is easier to generate the
      # key locally and copy to the remote than the opposite.
      fx-warn "No SSH credentials found, generating one locally first."
      # heck-ssh-config is defined in //tools/devshell/lib/vars.sh,
      # assume it is already loaded by the caller
      check-ssh-config  || exit $KEY_GENERATION_ERROR
      has_local_key=true
  fi

  if $has_local_key && $has_remote_key ; then
    # check if they match
    compare_remote_and_local || fail_with_mixed_mismatch
    return 0

  elif $has_local_key && ! $has_remote_key; then
    fx-warn "Copying local SSH credentials to the remote server"
    copy_local_to_remote

  elif ! $has_local_key && $has_remote_key; then
    fx-warn "Copying remote SSH credentials to the local machine"
    copy_remote_to_local
  fi
}
