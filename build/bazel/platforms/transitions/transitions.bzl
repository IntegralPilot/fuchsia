# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

visibility(["//build/bazel/rules/assembly"])

def _sdk_to_platform_transition_impl(settings, _attr):
    current_platforms = settings["//command_line_option:platforms"]
    if len(current_platforms) != 1:
        fail("Found multiple platforms configured: %s" % current_platforms)

    platform = str(current_platforms[0])
    if platform.startswith("//build/bazel/platforms:fuchsia_sdk_") or \
       platform.startswith("@//build/bazel/platforms:fuchsia_sdk_") or \
       platform.startswith("@@//build/bazel/platforms:fuchsia_sdk_"):
        # If the current platform is a Fuchsia SDK platform, derive the target platform
        # by replacing "_sdk_" with "_platform_".
        return {"//command_line_option:platforms": [platform.replace("_sdk_", "_platform_", 1)]}

    fail("This is only for transitioning from Fuchsia SDK targets to the platform.  Found: %s" % platform)

sdk_to_platform_transition = transition(
    implementation = _sdk_to_platform_transition_impl,
    inputs = ["//command_line_option:platforms"],
    outputs = ["//command_line_option:platforms"],
)
