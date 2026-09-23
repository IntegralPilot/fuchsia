# Copyright 2019 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Determines if a package server can be started.
function check-if-we-can-start-package-server {
  local name="$1"
  local expected_ip=$2
  local expected_port=$3

  # We can run if nothing is listening on our port.
  if ! is-listening-on-port "${expected_port}"; then
    return 0
  fi

  expected_addr=$(join-repository-ip-port "${expected_ip}" "${expected_port}")
  local err=$?
  if [[ "${err}" -ne 0 ]]; then
    return 1
  fi

  # Check if the ffx package repository server is already running on the expected address.
  actual_addr=$(ffx-repository-server-running-address "$name")
  local err=$?
  if [[ "${err}" -ne 0 ]]; then
    return 1
  fi

  if [[ -n "${actual_addr}" ]]; then
    if [[ "${expected_addr}" == "${actual_addr}" ]]; then
      return 0
    else
      fx-error "The repository server is already running on '${actual_addr}', not '${expected_addr}'."
      fx-error "To fix this, run:"
      fx-error ""
      fx-error "$ ffx repository server stop"
      fx-error ""
      fx-error "Then re-run this command."

      return 1
    fi
  else
    fx-error "Another process is using port '${expected_port}', which"
    fx-error "will block the ffx repository server from listening on ${expected_addr}."
    fx-error ""
    fx-error "Try shutting down that process, and re-running this command."

    return 1
  fi

}

function check-for-package-server {

  # Check for a server serving the same repo
  # Remove the trailing slash to prevent grepping against a directory with two slashes in a row.
  running_info="$(fx-command-run ffx --machine json repository server list | grep "${FUCHSIA_BUILD_DIR%/}/amber-files")"
  if [[ "${running_info}" == "" ]]; then
        fx-error "It looks like the package repository server is not running."
        fx-error "You probably need to run \"fx serve\""
        return 1
  fi

  return 0
}

function is-listening-on-port {
  local port=$1

  if [[ "$(uname -s)" == "Darwin" ]]; then
    if netstat -anp tcp | grep -v TIME_WAIT | grep -v FIN_WAIT1 | awk '{print $4}' | grep "\.${port}$" > /dev/null; then
      return 0
    fi
  else
    # List all open tcp sockets (excluding freshly disconnected ones); extract
    # their local address; check whether any port matches the argument.
    if ss -t -f inet -f inet6 -an exclude time-wait exclude fin-wait-1 | awk '{print $4}' | grep ":${port}$" > /dev/null; then
      return 0
    fi
  fi

  return 1
}

# If the server is running, this returns the address the server is running on.
# Otherwise it returns an empty string.
function ffx-repository-server-running-address {
  server_name="$1"
  if [[ "$server_name" != "" ]]; then
    name_filter="--name $server_name"
  else
    name_filter=""
  fi
  address=$(
    fx-command-run ffx --machine json repository server list "${name_filter}" |
      fx-command-run jq -r '.ok.data[].address'
  )
  err=$?
  if [[ "${err}" -ne 0 ]]; then
    fx-error "Unable to get the active ffx repository server list."
    fx-error "Current server list: $(ffx --machine json-pretty repository server list)"
    return "${err}"
  fi

  echo "${address}"

  return 0
}

# If the server is running, this returns the port the server is running on.
# Otherwise it returns an empty string.
function ffx-repository-server-running-port {
  name="$1"
  addr=$(ffx-repository-server-running-address "$name")
  err=$?
  if [[ "${err}" -ne 0 ]]; then
    fx-error "Unable to get the running ffx repository server address."
    return "${err}"
  fi

  # Don't return anything if the server is not running.
  if [[ -z "${addr}" ]]; then
    return 0
  fi

  if [[ ${addr} =~ .*:([0-9]+) ]]; then
    echo "${BASH_REMATCH[1]}"
  else
    fx-error "Could not parse port from ffx repository server address: '$addr'"
    fx-error "Current serrvers: $(ffx repository server list)"
    return 1
  fi

  return 0
}

function ffx-configured-repository-server-address {
  addr=$(fx-command-run ffx config get repository.server.listen)
  err=$?
  if [[ "${err}" -ne 0 ]]; then
    fx-error "Unable to get the configured repository server address."
    return "${err}"
  fi

  if [[ "${addr}" = "null" ]]; then
    echo ""
  else
    # Regex: Remove the leading and trailing quotes from the address.
    if [[ $addr =~ \"(.*)\" ]]; then
      echo "${BASH_REMATCH[1]}"
    else
      fx-error "could not parse ffx server address: '${addr}'"
      return 1
    fi
  fi

  return 0
}

function ffx-configured-repository-server-port {
  addr=$(ffx-configured-repository-server-address)
  err=$?
  if [[ "${err}" -ne 0 ]]; then
    fx-error "Unable to get the configured repository server address."
    return "${err}"
  fi

  if [[ ${addr} =~ .*:([0-9]+) ]]; then
    echo "${BASH_REMATCH[1]}"
  else
    fx-error "could not parse port from ffx server address: '$addr'"
    return 1
  fi

  return 0
}

function default-repository-url {

    ffx_repo="$(ffx-default-repository-name)" || return $?
    echo "fuchsia-pkg://${ffx_repo}"

  return 0
}

function join-repository-ip-port {
  local expected_ip="$1"
  local expected_port="$2"

  configured_addr=$(ffx-configured-repository-server-address)
  local err=$?
  if [[ "${err}" -ne 0 ]]; then
    fx-error "Could not read ffx repository server from config"
    return "${err}"
  fi

  if [[ $configured_addr =~ (.*):([0-9]+) ]]; then
    local configured_ip="${BASH_REMATCH[1]}"
    local configured_port="${BASH_REMATCH[2]}"
  else
    fx-error "could not parse ip and port from the configured ffx repository server address: '$configured_addr'"
    return 1
  fi

  if [[ -z "${expected_ip}" ]]; then
    expected_ip="${configured_ip}"
  elif [[ ${expected_ip} =~ : ]]; then
    expected_ip="[${expected_ip}]"
  fi

  if [[ -z "${expected_port}" ]]; then
    expected_port="${configured_port}"
  fi

  echo "${expected_ip}:${expected_port}"

  return 0
}
