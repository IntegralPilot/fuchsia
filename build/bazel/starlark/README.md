# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

This directory contains a number of small starlark files that can be used
in `bazel cquery` invocations to extract information about the Bazel
build graph. For example:

```
fx bazel cquery <config-options>
    --output=starlark --starlark:file=/path/to/this/file \
    //path/to:target
```

They are mostly an implementation detail of the `bazel_gn_target_action.py` and
`bazel_action_impl.py` scripts. Their content may change arbitrarily to keep
track of internal changes within the Bazel SDK rules.
