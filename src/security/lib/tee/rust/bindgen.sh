#!/bin/bash

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

usage () {
  echo "Usage: bindgen.sh <fuchsia root directory>"
}

if [ -z "$1" ]
then
  usage
  exit -1
fi

readonly FUCHSIA_HOME=$1
readonly OUT_PATH=${FUCHSIA_HOME}/src/security/lib/tee/rust/src/tee_client_api.rs

${FUCHSIA_HOME}/prebuilt/third_party/rust_bindgen/linux-x64/bindgen ${FUCHSIA_HOME}/src/security/lib/tee/tee-client-api/include/tee-client-api/tee_client_api.h \
  -o ${OUT_PATH} --no-layout-tests -- \
  -I${FUCHSIA_HOME}/zircon/system/public -I${FUCHSIA_HOME}/src/security/lib/tee/tee-client-api/include

TMP="$(mktemp)"

# Prepend copyright comment, #[allow] for various warnings we don't care about,
# and a line telling Rust to link against tee-client-api.
cat >> "$TMP" <<EOF
// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

EOF

cat ${OUT_PATH} >> "$TMP"
mv "$TMP" ${OUT_PATH}
