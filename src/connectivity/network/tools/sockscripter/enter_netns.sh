#!/bin/bash
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Running this script drops you into a shell inside of a network namespace.
#
# By default, the network namespace is named "sockscripter_netns", but you can pass
# a single argument to the script to choose a different netns name.
# Inside of this shell, you'll have the ability to run most `ip` commands
# (including ones that add and manipulate interfaces/addresses) without `sudo`.
#
# Also, `fx` commands (including `fx sockscripter`) will still work.

set -x

NETNS=${1:-"sockscripter_netns"}

if ! ip netns ls | grep -qFx "$NETNS"; then
  sudo ip netns add "$NETNS"
fi

export -p > /tmp/savedenv

# SYS_ADMIN is needed in order to allow us to `ip netns exec` without asking for
# `sudo` (and thus typing in the password) twice in the same invocation.
CAPS="+NET_RAW,+NET_ADMIN,+SYS_ADMIN"
sudo setpriv --inh-caps=$CAPS --ambient-caps=$CAPS --bounding-set=$CAPS \
  --reuid="$(id -u "$(whoami)")" --init-groups \
  /bin/bash -c "source /tmp/savedenv; rm /tmp/savedenv; ip netns exec $NETNS /usr/bin/env $SHELL"
