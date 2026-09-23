#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

function generate-ssh-config {
  # $1 is private key #2 is the location to write the file.
  priv_key="$1"
  sshconfig="$2"
  identities_only="${3:-no}"

  mkdir -p "$(dirname "$sshconfig")"

  cat <<EOF >"$sshconfig"
Host 127.0.0.1
  Port 8022

Host ::1
  Port 8022

Host *
  IdentitiesOnly $identities_only
  CheckHostIP no
  StrictHostKeyChecking no
  ForwardAgent no
  ForwardX11 no
  UserKnownHostsFile /dev/null
  IdentityFile $priv_key
  ControlPersist yes
  ControlMaster auto

  # When expanded, the ControlPath below cannot have more than 90 characters
  # (total of 108 minus 18 used by a random suffix added by ssh).
  # '%C' expands to 40 chars and there are 9 fixed chars, so '~' can expand to
  # up to 41 chars, which is a reasonable limit for a user's home in most
  # situations. If '~' expands to more than 41 chars, the ssh connection
  # will fail with an error like:
  #     unix_listener: path "..." too long for Unix domain socket
  # A possible solution is to use /tmp instead of ~, but it has
  # its own security concerns.
  ControlPath ~/.ssh/fx-%C

  ConnectTimeout 10
  ServerAliveInterval 1
  ServerAliveCountMax 10
  LogLevel ERROR
  PubkeyAuthentication yes
  PasswordAuthentication no
EOF
}