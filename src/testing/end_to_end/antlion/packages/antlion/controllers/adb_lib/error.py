#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion import error


class AdbError(error.ActsError):
    """Raised when there is an error in adb operations."""

    def __init__(self, cmd, stdout, stderr, ret_code):
        super().__init__()
        self.cmd = cmd
        self.stdout = stdout
        self.stderr = stderr
        self.ret_code = ret_code

    def __str__(self):
        return (
            "Error executing adb cmd '%s'. ret: %d, stdout: %s, stderr: %s"
        ) % (
            self.cmd,
            self.ret_code,
            self.stdout,
            self.stderr,
        )


class AdbCommandError(AdbError):
    """Raised when there is an error in the command being run through ADB."""
