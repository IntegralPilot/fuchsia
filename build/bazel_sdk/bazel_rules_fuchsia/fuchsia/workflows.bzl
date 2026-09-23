# Copyright 2021 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Public definitions for Fuchsia rules.

Documentation for all rules exported by this file is located at docs/workflows.md"""

load(
    "//fuchsia/private/workflows:fuchsia_shell_task.bzl",
    _fuchsia_shell_task = "fuchsia_shell_task",
    _shell_task_rule = "shell_task_rule",
)
load(
    "//fuchsia/private/workflows:fuchsia_task.bzl",
    _fuchsia_task = "fuchsia_task",
    _fuchsia_task_rule = "fuchsia_task_rule",
)
load(
    "//fuchsia/private/workflows:fuchsia_task_ffx.bzl",
    _ffx_task_rule = "ffx_task_rule",
    _fuchsia_task_ffx = "fuchsia_task_ffx",
)
load("//fuchsia/private/workflows:fuchsia_task_verbs.bzl", _verbs = "verbs")
# load(
#     "//fuchsia/private/workflows:fuchsia_remote_product_bundle.bzl",
#     _fuchsia_remote_product_bundle = "fuchsia_remote_product_bundle",
# )
# load(
#     "//fuchsia/private/workflows:fuchsia_task_launch_emulator.bzl",
#     _fuchsia_task_launch_emulator = "fuchsia_task_launch_emulator",
# )
# load(
#     "//fuchsia/private/workflows:fuchsia_task_flash.bzl",
#     _fuchsia_task_flash = "fuchsia_task_flash",
# )
# load(
#     "//fuchsia/private/workflows:fuchsia_task_publish.bzl",
#     _fuchsia_task_publish = "fuchsia_task_publish",
# )
# load(
#     "//fuchsia/private/workflows:fuchsia_task_run_component.bzl",
#     _fuchsia_task_run_component = "fuchsia_task_run_component",
# )

load(
    "//fuchsia/private/workflows:fuchsia_workflow.bzl",
    _fuchsia_workflow = "fuchsia_workflow",
)

# Workflow build rules.
fuchsia_task = _fuchsia_task
fuchsia_workflow = _fuchsia_workflow
fuchsia_shell_task = _fuchsia_shell_task
fuchsia_task_ffx = _fuchsia_task_ffx
verbs = _verbs

# TODO(https://fxbug.dev/42064517): Expose these rules once implementation/API is complete.

# fuchsia_remote_product_bundle = _fuchsia_remote_product_bundle
# fuchsia_task_flash = _fuchsia_task_flash
# fuchsia_task_launch_emulator = _fuchsia_task_launch_emulator
# fuchsia_task_publish = _fuchsia_task_publish
# fuchsia_task_run_component = _fuchsia_task_run_component

# Starlark utilities for defining new tasks.
fuchsia_task_rule = _fuchsia_task_rule
shell_task_rule = _shell_task_rule
ffx_task_rule = _ffx_task_rule
