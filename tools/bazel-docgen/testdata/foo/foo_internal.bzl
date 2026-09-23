# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

def _example_rule_impl(_ctx):
    return []

example_rule = rule(
    implementation = _example_rule_impl,
    doc = """The description of a basic rule

    Some other text on a new line.
    """,
    attrs = {
        "deps": attr.label_list(mandatory = False, doc = "the dependencies."),
        "src": attr.label(doc = "A source file", mandatory = True, allow_single_file = True),
        "count": attr.int(),
        "mapping": attr.string_dict(),
    },
)

def _empty_repo_impl(_ctx):
    pass

empty_repo = repository_rule(
    implementation = _empty_repo_impl,
    doc = "Just an empty repository rule",
    environ = ["FOO_ENV_VAR"],
)
