# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Aspect to print output paths of buildfiles_genquery targets."""

def _genquery_output_file_impl(target, ctx):
    # We only care about targets in buildfiles_genquery package
    if target.label.package == "buildfiles_genquery":
        for file in target.files.to_list():
            # Print to stderr with a prefix that can be filtered
            # LINT.IfChange(buildfiles_genquery_prefix)
            print("BUILDFILES_GENQUERY_OUTPUT_FILE=%s,%s" % (target.label, file.path))
            # LINT.ThenChange(//build/bazel/scripts/bazel_action_impl.py)

    return []

genquery_output_file = aspect(
    doc = """Aspect that prints the output paths of genquery targets.

This aspect is used by bazel_action_impl.py to discover the output paths
of generated buildfiles_genquery targets without relying on the bazel-bin symlink.
""",
    implementation = _genquery_output_file_impl,
)
