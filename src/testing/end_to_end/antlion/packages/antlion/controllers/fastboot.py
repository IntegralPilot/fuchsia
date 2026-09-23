#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

# Blanket ignores to enable mypy in Antlion
# mypy: disable-error-code="no-untyped-def"
from antlion import error
from libs.proc import job


class FastbootError(error.ActsError):
    """Raised when there is an error in fastboot operations."""

    def __init__(self, cmd, stdout, stderr, ret_code):
        super().__init__()
        self.cmd = cmd
        self.stdout = stdout
        self.stderr = stderr
        self.ret_code = ret_code

    def __str__(self):
        return (
            "Error executing fastboot cmd '%s'. ret: %d, stdout: %s,"
            " stderr: %s"
        ) % (self.cmd, self.ret_code, self.stdout, self.stderr)


class FastbootProxy:
    """Proxy class for fastboot.

    For syntactic reasons, the '-' in fastboot commands need to be replaced
    with '_'. Can directly execute fastboot commands on an object:
    >> fb = FastbootProxy(<serial>)
    >> fb.devices() # will return the console output of "fastboot devices".
    """

    def __init__(self, serial="", ssh_connection=None):
        self.serial = serial
        if serial:
            self.fastboot_str = f"fastboot -s {serial}"
        else:
            self.fastboot_str = "fastboot"
        self.ssh_connection = ssh_connection

    def _exec_fastboot_cmd(
        self, name, arg_str, ignore_status=False, timeout=60
    ):
        command = f"{self.fastboot_str} {name} {arg_str}"
        if self.ssh_connection:
            result = self.ssh_connection.run(
                command, ignore_status=True, timeout_sec=timeout
            )
        else:
            result = job.run(command, ignore_status=True, timeout_sec=timeout)
        ret, out, err = result.exit_status, result.stdout, result.stderr
        # TODO: This is only a temporary workaround for b/34815412.
        # fastboot getvar outputs to stderr instead of stdout
        if "getvar" in command:
            out = err
        if ret == 0 or ignore_status:
            return out
        else:
            raise FastbootError(
                cmd=command, stdout=out, stderr=err, ret_code=ret
            )

    def args(self, *args, **kwargs):
        return job.run(" ".join((self.fastboot_str,) + args), **kwargs).stdout

    def __getattr__(self, name):
        def fastboot_call(*args, **kwargs):
            clean_name = name.replace("_", "-")
            arg_str = " ".join(str(elem) for elem in args)
            return self._exec_fastboot_cmd(clean_name, arg_str, **kwargs)

        return fastboot_call
