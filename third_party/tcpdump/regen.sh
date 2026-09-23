#!/bin/bash
# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -euxo pipefail

readonly REPO_DIR="$FUCHSIA_DIR/third_party/tcpdump"
TCPDUMP_TAG="tcpdump-$(cat "$REPO_DIR/src/RELEASE_VERSION")"
readonly TCPDUMP_TAG

readonly CONFIG_H="$REPO_DIR/config.h"

"$FUCHSIA_DIR"/scripts/autoconf/regen.sh \
  FUCHSIA_OUT_CONFIG_H="${CONFIG_H}.fuchsia" \
  LINUX_OUT_CONFIG_H="${CONFIG_H}.linux" \
  FXBUILD_WITH_ADDITIONAL="third_party/libpcap" \
  CPPFLAGS_ADDITIONAL="-I$FUCHSIA_DIR/third_party/libpcap/src" \
  LDFLAGS_ADDITIONAL="-lpcap" \
  LINUX_LIBRARY="third_party/libpcap" \
  REPO_ZIP_URL="https://github.com/the-tcpdump-group/tcpdump/archive/refs/tags/$TCPDUMP_TAG.zip" \
  REPO_EXTRACTED_FOLDER="tcpdump-$TCPDUMP_TAG" \
  CONFIGURE_ARGS_FUCHSIA="--without-crypto ac_cv_func_fork=no ac_cv_func_getservent=no" \
  CONFIGURE_ARGS_LINUX="--without-crypto"
