# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#!/usr/bin/env python3
import subprocess

# Suppose we want a simple test that does a health check by
# validating that we can communicate with the target, such as:

out = subprocess.run(
    ["ffx", "target", "echo", "foo"], check=True, capture_output=True
)
assert out.stdout == b"foo", f"stdout is {out.stdout}, not 'foo'"

# Simple, but there are problems:
#
# 1. It doesn't work!
# The output of `ffx target echo foo` is 'SUCCESS: received "foo"\n', not simply
# "foo". It would be better in a test to check the machine output.

# 2. The test relies on the daemon.
# Depending on the state of the environment, this may start a daemon, or not.
# So this doesn't just test the target, it also tests the environment.
# We can run `ffx` with `--isolate-dir <dir>` but that adds even more complexity.

# 3. It doesn't specify the target.
# Exactly what this test does depends on the environment in which it is run.
# Perhaps it uses the default target? Or does an mDNS broadcast? Or prompts the
# daemon for targets? Debugging a failure can be an exercise in frustration under
# these circumstances.
# Furthermore, some IT environments discourage the use of mDNS. It is not clear
# where or in what circumstances this test would violate such a policy.

# 4. The behavior depends on the configuration.
# Suppose a builder fails, but when the developer runs the script, it works for them.
# How can they know what exactly it is trying to do? Perhaps there is a configuration
# option being set (such as `proxy.timeout_secs`) that the developer set and then forgot
# about.

# 5. It will write a log file
# Without any indication to the user, this will write a log file (by default in their
# home directory.) This is fine when run locally, but in a test environment, this is
# a bug waiting to happen.
