# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Bazel Action Keywords for rules that must be executed locally."""

# A dictionary to be expanded inside a ctx.actions.run() or
# ctx.actions.run_shell() call to specify that the corresponding
# action should only run locally.
#
# Example usage is:
#
#    ctx.actions.run(
#      executable = ...,
#      inputs = ...,
#      outputs = ....
#      **LOCAL_ONLY_ACTION_KWARGS
#    )
#
# A good reason to use this is to avoid sending very large
# input or outputs through the network, especially when
# running the command locally can still be fast.
#
# IMPORTANT: This does NOT disable Bazel sandboxing, like
# the Bazel "local" tag does.
#
# See https://bazel.build/reference/be/common-definitions#common-attributes
#
LOCAL_ONLY_ACTION_KWARGS = {
    "execution_requirements": {
        "no-remote": "1",
        "no-cache": "1",
    },
}
