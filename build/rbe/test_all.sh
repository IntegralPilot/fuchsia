#!/bin/bash -ev
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Manually run all python unit-tests in this dir.
# This is faster than `fx test //build/rbe`.
# Some tests require python protobufs to be compiled first,
# those are wrapped in a .sh script that adjusts
# PYTHONPATH accordingly.

readonly script="$0"
# assume script is always with path prefix, e.g. "./$script"
readonly script_dir="${script%/*}"  # dirname
readonly script_basename="${script##*/}"  # basename

readonly rbe_dir="$script_dir"

for t in "$rbe_dir"/*_test.py
do
  stem="${t##*/}"
  base="${stem%.py}"
  sh_test="$rbe_dir/$base.sh"
  if test -f "$sh_test"
  then
    echo "---- $sh_test ----"
    ./"$sh_test"
  else
    echo "---- $t ----"
    ./"$t"
  fi
done
