# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

_DEP_ATTRS = [
    "deps",
    "implementation_deps",
    "data",
    "compile_data",
]

TransitiveDepsInfo = provider(
    doc = "Information about transitive dependencies for assert_no_deps aspect.",
    fields = {
        "labels": "depset of Labels of transitive dependencies (including self).",
    },
)

def _assert_no_deps_aspect_impl(_target, ctx):
    child_labels = []
    for attr_name in _DEP_ATTRS:
        for dep in getattr(ctx.rule.attr, attr_name, []):
            if TransitiveDepsInfo not in dep:
                continue
            child_labels.append(dep[TransitiveDepsInfo].labels)

    transitive_deps = depset(transitive = child_labels)

    assert_no_deps = []
    tags = getattr(ctx.rule.attr, "tags", [])
    for tag in tags:
        if tag.startswith("assert_no_deps="):
            assert_no_deps.append(tag[len("assert_no_deps="):])

    if assert_no_deps:
        # Only flatten the depset if there are assert_no_deps specified, because flattening a depset
        # is a costly operation and we should avoid it if possible.
        #
        # Most `assert_no_deps` consists of one or a few entries, so building up a lookup dictionary
        # is not worth it.
        transitive_deps_list = transitive_deps.to_list()

        violated_labels = []
        for no_dep_label_str in assert_no_deps:
            # Convert to Label for proper label comparisons below.
            no_dep_label = ctx.label.relative(no_dep_label_str)

            # Support `//foo/bar:__pkg__`, which is equivalent to GN's `//foo/bar:*`.
            if no_dep_label.name == "__pkg__":
                for dep in transitive_deps_list:
                    if no_dep_label.package == dep.package:
                        violated_labels.append(no_dep_label_str)
                        break
                continue

            # Support `//foo/bar:__subpackages__`, which is equivalent to GN's `//foo/bar/*`.
            if no_dep_label.name == "__subpackages__":
                for dep in transitive_deps_list:
                    if dep.package.startswith(no_dep_label.package):
                        violated_labels.append(no_dep_label_str)
                        break
                continue

            # For exact label matches.
            if no_dep_label in transitive_deps_list:
                violated_labels.append(no_dep_label_str)
                continue

            # A special case to handle generated aliases under //third_party/rust_crates.
            #
            # For example, given how Bazel handles aliases in deps, the alias
            # `//third_party/rust_crates/vendor:anyhow` will be canonicalized to
            # "//third_party/rust_crates/vendor/anyhow-1.0.102:anyhow".
            if no_dep_label.package.startswith("third_party/rust_crates/vendor"):
                for dep in transitive_deps_list:
                    if dep.name == no_dep_label.name and dep.package.startswith(no_dep_label.package):
                        violated_labels.append(no_dep_label_str)
                        break
                continue

        if violated_labels:
            fail("Target {} violates assert_no_deps: found forbidden dependencies: {}".format(ctx.label, ", ".join(violated_labels)))

    return [
        TransitiveDepsInfo(
            labels = depset([ctx.label], transitive = [transitive_deps]),
        ),
    ]

assert_no_deps_aspect = aspect(
    implementation = _assert_no_deps_aspect_impl,
    attr_aspects = _DEP_ATTRS,
)
