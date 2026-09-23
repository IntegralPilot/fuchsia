# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Generic Git staging isolation and staged action runner package."""

from __future__ import annotations

from .engine import (
    ActionContext,
    GitError,
    PipelineResult,
    StagedFileAction,
    StagedPartition,
    StashGuard,
    partition_staged_files,
    run_staged_pipeline,
)

__all__ = [
    "ActionContext",
    "GitError",
    "PipelineResult",
    "StagedFileAction",
    "StagedPartition",
    "StashGuard",
    "partition_staged_files",
    "run_staged_pipeline",
]
