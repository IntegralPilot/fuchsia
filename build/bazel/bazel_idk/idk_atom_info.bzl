# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Debugging aspect for IDK atoms."""

load("//build/bazel/rules/idk/private:providers.bzl", "FuchsiaIdkAtomInfo")

def _print_deps_aspect_impl(_target, ctx):
    # buildifier: disable=print
    print("\n", ctx.rule.attr.name, ":\n\ttype: ", ctx.rule.attr.type, "\n\tIDK deps: ", ctx.rule.attr.deps, "\n\tnon-IDK build deps: ", ctx.rule.attr.atom_build_deps)
    return []

print_deps_aspect = aspect(
    doc = """An aspect that prints information about the atom and all the atoms
on which it depends. It is only for debugging and demonstration purposes.
Example use:
    fx bazel build --config=fuchsia_platform build/bazel/bazel_idk/tests:test-source-library_idk "--aspects=build/bazel/bazel_idk/idk_atom_info.bzl%print_deps_aspect"
""",
    implementation = _print_deps_aspect_impl,
    attr_aspects = ["deps"],
    required_providers = [FuchsiaIdkAtomInfo],
)
