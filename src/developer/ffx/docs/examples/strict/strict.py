# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

#!/usr/bin/env python3
import os

import ffx

# Let's assume there's a wrapper class, that might be a standard library made
# available for writing ffx-based tests:


# Given such a wrapper, we can use "ffx --strict". We have to specify
# a couple things that were not necessary before:
# * We must provide the `ssh.priv` file, because it will not be read for
#   the environment.
# * We must say where any logs should go.
#
# In return, our test is now almost as simple, but:
# * It does not use the daemon.
# * This version requires the target to be specified. Alternatively, we could
#   discover a default target, but that discovery would be _explicit_ and under
#   the test's control.
# * The only configurations we override are those required by ffx.
# * The location of the log file is explicit (and required).

# Run with:
# FUCHSIA_NODENAME=<target-ip:port> python3 strict.py

ssh_key = os.environ.get(
    "FUCHSIA_SSH_KEY", os.path.expanduser("~/.ssh/fuchsia_ed25519")
)
# Instantiate the runner. If FUCHSIA_NODENAME is not provided (is None),
# the runner constructor will automatically discover the default target.
target_name = os.environ.get("FUCHSIA_NODENAME")
runner = ffx.FfxRunner("my.log", ssh_key, target_name)

# Print the output from `ffx target echo Hello`.
print("Running ffx target echo Hello in strict mode:")
out = runner.target_echo("Hello")
print(out)
print()

# Make sure that the returned message is "Hello".
assert out == "Hello", f"expected:\n\t'Hello'\ngot:\n\t'{out}'"

# Print the output from `ffx target list`.
print("Running ffx target list in strict mode:")
target_list = runner.target_list(None)
print(target_list)
print()

# Print the output from `ffx target show`
print("Running ffx target show in strict mode:")
target_show = runner.target_show()
print(target_show)
print()


# Print the number of components returned from `ffx component list`
print("Running ffx component list in strict mode:")
component_list = runner.component_list()
print(f"{len(component_list)} components are found on this target device.")
print()
