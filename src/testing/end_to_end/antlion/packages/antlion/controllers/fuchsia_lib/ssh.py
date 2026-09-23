#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import time

from antlion.capabilities.ssh import SSHProvider
from libs.proc.runner import CalledProcessError

DEFAULT_SSH_USER: str = "fuchsia"
DEFAULT_SSH_PRIVATE_KEY: str = "~/.ssh/fuchsia_ed25519"
# The default package repository for all components.
FUCHSIA_PACKAGE_REPO_NAME = "fuchsia.com"


class FuchsiaSSHProvider(SSHProvider):
    """Device-specific provider for SSH clients."""

    def start_v1_component(
        self,
        component: str,
        timeout_sec: int = 5,
        repo: str = FUCHSIA_PACKAGE_REPO_NAME,
    ) -> None:
        """Start a CFv1 component in the background.

        Args:
            component: Name of the component without ".cmx".
            timeout_sec: Seconds to wait for the process to show up in 'ps'.
            repo: Default package repository for all components.

        Raises:
            TimeoutError: when the component doesn't launch within timeout_sec
        """
        # The "run -d" command will hang when executed without a pseudo-tty
        # allocated.
        self.config.force_tty = True
        self.run(
            f"run -d fuchsia-pkg://{repo}/{component}#meta/{component}.cmx",
        )
        self.config.force_tty = False

        timeout = time.perf_counter() + timeout_sec
        while True:
            ps_cmd = self.run("ps")
            if f"{component}.cmx" in ps_cmd.stdout.decode("utf-8"):
                return
            if time.perf_counter() > timeout:
                raise TimeoutError(
                    f'Failed to start "{component}.cmx" after {timeout_sec}s'
                )

    def stop_component(
        self, component: str, is_cfv2_component: bool = False
    ) -> None:
        """Stop all instances of a CFv1 or CFv2 component.

        Args:
            component: Name of the component without suffix("cm" or "cmx").
            is_cfv2_component: Determines the component suffix to use.
        """
        suffix = "cm" if is_cfv2_component else "cmx"

        try:
            self.run(["killall", f"{component}.{suffix}"])
            self.log.info(f"Stopped component: {component}.{suffix}")
        except CalledProcessError as e:
            if b"no tasks found" in e.stderr:
                self.log.debug(
                    f"Could not find component: {component}.{suffix}"
                )
                return
            raise e
