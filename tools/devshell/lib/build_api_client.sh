# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Wrapper function to call //build/api/client from fx scripts.
# This should be the only way to invoke that script from `fx`, as it
# ensures the build directory is always correctly set (https://fxbug.dev/336720162)
# $1+: command arguments.
fx-build-api-client () {
  "${FUCHSIA_DIR}/build/api/client" --build-dir="${FUCHSIA_BUILD_DIR}" "$@"
}
