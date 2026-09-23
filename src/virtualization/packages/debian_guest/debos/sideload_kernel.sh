#!/bin/sh
#
# Copyright 2026 The Fuchsia Authors
#
# Use of this source code is governed by a MIT-style
# license that can be found in the LICENSE file or at
# https://opensource.org/licenses/MIT

# This script is run within the chroot of the created Debian distribution.
set -e

if [ "$#" -eq 0 ]; then
    echo "Error: No URL provided for kernel sideload operation." >&2
    exit 1
fi

# Download the specific signed image
wget -O /tmp/sideloaded-kernel.deb $1

# Install it using dpkg (this places vmlinuz in /boot and modules in /lib/modules)
# We use --force-depends if there are minor version mismatches in secondary tools
dpkg -i --force-depends /tmp/sideloaded-kernel.deb

# Clean up the deb file to keep the image small
rm /tmp/sideloaded-kernel.deb