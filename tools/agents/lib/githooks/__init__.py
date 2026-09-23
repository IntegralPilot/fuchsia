# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Fuchsia Git hooks and reporting subsystem."""

from __future__ import annotations

from .adapters import (
    DEFAULT_FORMATTABLE_EXTENSIONS,
    DEFAULT_PRE_COMMIT_ACTIONS,
    HookAction,
    HookContext,
    commit_msg_action,
    format_code_action,
)
from .installer import (
    HookOperationResult,
    HookStatusResult,
    get_git_hooks_status,
    install_git_hook,
    install_git_hooks,
    uninstall_git_hook,
    uninstall_git_hooks,
)
from .reporters import (
    ConsoleReporter,
)
from .runner import (
    main,
    run_commit_msg_hook,
    run_pre_commit_hook,
)

__all__ = [
    "ConsoleReporter",
    "DEFAULT_FORMATTABLE_EXTENSIONS",
    "DEFAULT_PRE_COMMIT_ACTIONS",
    "HookAction",
    "HookContext",
    "HookOperationResult",
    "HookStatusResult",
    "commit_msg_action",
    "format_code_action",
    "get_git_hooks_status",
    "install_git_hook",
    "install_git_hooks",
    "main",
    "run_commit_msg_hook",
    "run_pre_commit_hook",
    "uninstall_git_hook",
    "uninstall_git_hooks",
]
