# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Public definitions for the foo library"""

load(
    "//tools/bazel-docgen/testdata/foo:foo_internal.bzl",
    _empty_repo = "empty_repo",
    _example_rule = "example_rule",
)

## Verify that we can grab targets that are imported into the file
example_rule = _example_rule
empty_repo = _empty_repo

def some_function(name, some_val = "some_default", some_int = 1):
    """ a starlark function

    Args:
      name: the name to pass
      some_val: some value to supply
      some_int: some integer value

    Returns:
      The number 1
    """
    return 1

def empty_function():
    """ empty function """
    pass

FooInfo = provider(
    doc = "Some provider",
    fields = {
        "contents": "Some contents",
    },
)
