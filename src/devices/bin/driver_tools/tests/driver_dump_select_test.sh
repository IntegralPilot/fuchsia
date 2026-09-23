#!/bin/bash
# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# NOTE: You need to include
# //src/devices/bin/driver_tools/tests:start_driver_test_realm_and_hang in your
# build for this test to work correctly.

# This is a regression test for https://fxbug.dev/42064796 that tests that the `--select` flag to
# `ffx driver dump` works. Specifically, we are testing that we can connect to
# an instance of driver manager inside a Driver Test Realm.

set -eu

function cleanup() {
  # At the end of the test, we should clean up the test component.
  echo
  echo "== Cleaning up =="
  $FFX component stop "$COMPONENT_MONIKER"
  $FFX component destroy "$COMPONENT_MONIKER"
}
trap cleanup EXIT

# The build system will pass the path to the `ffx` executable as the first
# argument to the script.
FFX=$1
COMPONENT_MONIKER=/core/ffx-laboratory:start_driver_test_realm_and_hang

echo
echo "Using ffx binary at $FFX"

# First, we need to start a component that runs Driver Test Realm. This is a
# component specifically for this test that simply starts Driver Test Realm and
# then spins forever.
echo
echo "== Creating test component =="
$FFX component run \
  --recreate \
  "$COMPONENT_MONIKER" \
  fuchsia-pkg://fuchsia.com/start_driver_test_realm_and_hang#meta/start_driver_test_realm_and_hang.cm

# Poll until the Driver Test Realm appears in the list. The `--select` option
# will cause `ffx driver dump` to print a menu; we look for option 2 because
# options 0 and 1 are always taken by the real driver manager.
#
# We use `echo` to simulate user input; in this case we just pipe in option 0
# because we are waiting for the Driver Test Realm to come up, not testing the
# tool.
echo
echo "== Waiting for test component to appear =="
option=2
while ! echo 0 | $FFX driver dump --select | grep "$option.*start_driver_test_realm_and_hang/realm_builder"; do
  sleep 0.1
done

# Now we can test the tool itself by piping in the option number for the Driver
# Test Realm instance.
#
# `set -e` at the beginning of this script will cause the test overall to fail
# if `ffx driver dump` exits with an error.
echo
echo "== Running test =="
echo $option | $FFX driver dump --select
