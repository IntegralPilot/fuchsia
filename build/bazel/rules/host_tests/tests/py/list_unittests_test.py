#!/usr/bin/env fuchsia-vendored-python
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import json
import os
import subprocess
import sys

import python.runfiles.runfiles as runfiles

parser = argparse.ArgumentParser()
parser.add_argument(
    "--verbose", action="store_true", help="Print verbose output."
)
parser.add_argument(
    "test_script", help="Rlocation of host python test wrapper script."
)
args = parser.parse_args()

r = runfiles.Create()
test_location = r.Rlocation(args.test_script)
test_env = r.EnvVars()

if args.verbose:
    print(
        f"Found test at {test_location} \nEnvironment: {json.dumps(test_env, indent=2)}\n"
        + f"cwd = {os.getcwd()}\n",
        flush=True,
    )

output = subprocess.check_output(
    [test_location, "--list_host_python_unittests"], env=test_env, text=True
)
expected_output = (
    "TestWithUnittests.test_something\nTestWithUnittests.test_something_else\n"
)
if output != expected_output:
    print(
        f"ERROR: Unexpected output: [\n{output}\n] EXPECTED [\n{expected_output}\n]",
        file=sys.stderr,
    )
    sys.exit(1)

print("Test run succesfully!")
