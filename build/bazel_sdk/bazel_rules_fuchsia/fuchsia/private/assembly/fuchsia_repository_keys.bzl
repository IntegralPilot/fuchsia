# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Rule for collecting Fuchsia TUF repository keys."""

load(":providers.bzl", "FuchsiaRepositoryKeysInfo")

_VALID_KEY_FILES = {
    "root.json": None,
    "targets.json": None,
    "snapshot.json": None,
    "timestamp.json": None,
}

def _fuchsia_repository_keys(ctx):
    keys = ctx.files.keys
    if len(keys) == 0:
        fail("keys cannot be empty")

    invalid_keys = [f.basename for f in keys if f.basename not in _VALID_KEY_FILES]
    if len(invalid_keys) > 0:
        fail("Invalid key files: {}, key must be one of {}".format(invalid_keys, _VALID_KEY_FILES.keys()))

    dirs = {f.dirname: None for f in keys}.keys()
    if len(dirs) > 1:
        fail("All repository keys must come from the same directory, got multiple: {}".format(dirs))

    return [
        DefaultInfo(files = depset(direct = keys)),
        FuchsiaRepositoryKeysInfo(dir = dirs[0]),
    ]

fuchsia_repository_keys = rule(
    doc = """Rule for collecting Fuchsia TUF repository keys from the same directory.""",
    implementation = _fuchsia_repository_keys,
    provides = [FuchsiaRepositoryKeysInfo],
    attrs = {
        "keys": attr.label_list(
            doc = "A list of JSON files containing private keys, which can optionally contain the following files: root.json, targets.json, snapshot.json, timestamp.json.",
            allow_files = [".json"],
            mandatory = True,
        ),
    },
)
