# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# install_host_tools does nothing in Bazel right now. It exists to facilitate
# target syncing between GN and Bazel.
#
# `target_compatible_with = HOST_CONSTRAINTS` must be specified for bazel2gn to
# generate the correct condition statement.
# TODO(https://fxbug.dev/442025401): Consider implementing this within bazel2gn
# rather than requiring it at each call site.
def install_host_tools(
        name,
        tool_output_names,  # GN equivalent: `outputs`
        implementation_deps,  # GN equivalent: `deps`
        target_compatible_with,
        visibility = ["//visibility:private"]):
    pass
