#!/bin/bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# In order to use packet sockets, you need to have the net_raw capability.
# Running this script drops you into a shell that provides the net_raw
# capability ambiently by:
# 1. Saving the current set of environment variables to a temporary file
# 2. Using setpriv to enter a bash session with the needed ambient capability
# 3. Sourcing the saved environment variables

set -Eeuox pipefail

export -p > /tmp/savedenv

CAPS="+NET_RAW"

sudo setpriv --inh-caps=$CAPS --ambient-caps=$CAPS --bounding-set=$CAPS \
  --reuid="$(id -u $(whoami))" --init-groups \
  /bin/bash -c "source /tmp/savedenv; rm /tmp/savedenv; /usr/bin/env $SHELL"

