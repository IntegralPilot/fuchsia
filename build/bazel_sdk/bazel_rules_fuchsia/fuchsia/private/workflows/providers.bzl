# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""All Fuchsia Task Providers."""

load(
    "@fuchsia_rules_common//debug_symbols:providers.bzl",
    _FuchsiaDebugSymbolInfo = "FuchsiaDebugSymbolInfo",
)
load(
    "//fuchsia/private:providers.bzl",
    _FuchsiaComponentInfo = "FuchsiaComponentInfo",
    _FuchsiaDriverToolInfo = "FuchsiaDriverToolInfo",
    _FuchsiaPackagedComponentInfo = "FuchsiaPackagedComponentInfo",
    _FuchsiaProductBundleInfo = "FuchsiaProductBundleInfo",
    _FuchsiaProvidersInfo = "FuchsiaProvidersInfo",
)

FuchsiaTaskEntityInfo = provider(
    "The execution atomic within a workflow.",
    fields = {
        "task_runner": "The task's runner path.",
        "args": "A list of arguments to give the task runner.",
        "default_argument_scope": "The default scope of arguments to use for this task.",
    },
)

FuchsiaWorkflowEntityInfo = provider(
    "A sequence of tasks.",
    fields = {
        "sequence": "The sequence of tasks that need to be run.",
        "args": "A list of arguments to give tasks.",
    },
)

FuchsiaWorkflowInfo = provider(
    "All tasks + workflows that comprise the top-level workflow.",
    fields = {
        "entities": "A collection of tasks & workflows which comprise this workflow.",
        "entrypoint": "The entrypoint to this workflow.",
    },
)

FuchsiaComponentInfo = _FuchsiaComponentInfo
FuchsiaPackagedComponentInfo = _FuchsiaPackagedComponentInfo
FuchsiaDebugSymbolInfo = _FuchsiaDebugSymbolInfo
FuchsiaDriverToolInfo = _FuchsiaDriverToolInfo
FuchsiaProductBundleInfo = _FuchsiaProductBundleInfo
FuchsiaProvidersInfo = _FuchsiaProvidersInfo
