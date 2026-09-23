#!/usr/bin/env bash
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# This script returns a random set of API reviewers from the owners file.
usage() { echo "Usage: $0 [-c <reviewer count] [-f <username to filter out>]" 1>&2; exit 1; }

while getopts "c:f:" ARG; do
  case "${ARG}" in
    c)
      COUNT=${OPTARG}
      ;;
    f)
      # Omit a specified user.
      FILTER=('-e' "${OPTARG}")
      ;;
    *)
      usage
      ;;
  esac
done

COUNT="${COUNT:-3}"
# Omit comment lines, omit empty lines.
FILTER+=('-e' '#' '-e' '^$' '-v')
SCRIPT_SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" >/dev/null 2>&1 && pwd)"

# Choose COUNT reviewers out of the set of unique API reviewers.
grep "${FILTER[@]}" < "${SCRIPT_SRC_DIR}/OWNERS" |
  sort |
  uniq |
  shuf |
  head -n "${COUNT}"
