#!/bin/bash

# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

set -e

# Safely find the BOOTSTRAP_DIR even if we are a symlink
find_bootstrap_dir() {
  if [[ -L "${BASH_SOURCE[0]}" ]]; then
    SYMLINK_DIR=$(dirname "$(readlink "${BASH_SOURCE[0]}")")
    cd "$(dirname "${BASH_SOURCE[0]}")"
    BOOTSTRAP_DIR="$(cd "${SYMLINK_DIR}" && pwd)"
  else
    BOOTSTRAP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  fi
}

main() {
  echo "Downloading bazel"
  find_bootstrap_dir
  "${BOOTSTRAP_DIR}/bootstrap_bazel.sh" "$@"
  echo "Download complete."
  echo "Bootstrapping git hooks"
  "${BOOTSTRAP_DIR}/bootstrap_hooks.sh" "$@"
  echo "Git hooks complete."
}

main "$@"
