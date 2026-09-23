# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
A repository rule `fuchsia_icu_config_repository` generates an external repo
that contains git commit ID information about the third party ICU repositories
contained in @//third_party/icu.

Defines two constants:
-  `icu_flavors` which is a list of available icu flavors (ie, 'stable', 'latest')
-  `icu_commits` which is a dict of icu flavors to their respective commit ids.

- `default`(string): the detected git commit ID for
   `@//:third_party/icu_default`
- `latest`(string): the detected git commit ID for
   `@//:third_party/icu_latest`

This dict can be ingested by main build rules by using:

In WORKSPACE.bazel:

```
load ("//:bazel/icu/repository_rules.bzl:", "fuchsia_icu_config_repository")

fuchsia_icu_config_repository(name = "fuchsia_icu_config")
```

in BUILD files:

```
load("@fuchsia_icu_config//:constants.bzl", "icu_flavors")
```

"""

_CONSTANTS_BZL_TEMPLATE = """# AUTO_GENERATED - DO NOT EDIT!

icu_flavors = [ "default", "latest" ]

icu_commits = {icu_config}

"""

def _fuchsia_icu_config_impl(repo_ctx):
    icu_build_config_file = repo_ctx.path(Label("@//:" + repo_ctx.attr.icu_build_config_json))
    contents = repo_ctx.read(icu_build_config_file)
    icu_config = json.decode(contents)

    constants_bzl = _CONSTANTS_BZL_TEMPLATE.format(
        icu_config = icu_config,
    )

    repo_ctx.file("constants.bzl", constants_bzl)

    repo_ctx.file("WORKSPACE.bazel", """# DO NOT EDIT! Automatically generated.
workspace(name = "fuchsia_icu_config")
""")
    repo_ctx.file("BUILD.bazel", """# DO NOT EDIT! Automatically generated.
exports_files(glob(["**/*"]))""")

fuchsia_icu_config_repository = repository_rule(
    implementation = _fuchsia_icu_config_impl,
    doc = "Create a repository that contains ICU configuration information in its //:constants.bzl file.",
    attrs = {
        "icu_build_config_json": attr.string(
            doc = "Path to the icu configuration file., relative to the workspace root.",
            mandatory = False,
        ),
    },
)
