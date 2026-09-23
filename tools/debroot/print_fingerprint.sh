#!/bin/sh
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

DISTRO="$1"
RELEASE="$2"

if [[ "$DISTRO" = "debian" ]]; then
    RELEASE_URL="https://archive.debian.org/debian-archive/debian/dists/$RELEASE"
elif [[ "$DISTRO" = "ubuntu" ]]; then
    RELEASE_URL="http://us.archive.ubuntu.com/ubuntu/dists/$RELEASE"
else
    echo "expecting either debian or ubuntu for distro, got $DISTRO"
fi;

RELEASE_FILE=$(mktemp)
SIGNATURE_FILE=$(mktemp)

echo "fetching release file..."
curl "$RELEASE_URL/Release" > "$RELEASE_FILE" || exit 1
echo "fetching signature file..."
curl "$RELEASE_URL/Release.gpg" > "$SIGNATURE_FILE" || exit 1

GPG_OUTPUT=$(mktemp)
gpg --verify $SIGNATURE_FILE $RELEASE_FILE 2>$GPG_OUTPUT

echo "full gpg output:"
cat $GPG_OUTPUT

KEY_ID_LINES=$(cat $GPG_OUTPUT | grep "using " | head -n1)
echo
echo
echo "key id:"
echo ${KEY_ID_LINES: -8}
