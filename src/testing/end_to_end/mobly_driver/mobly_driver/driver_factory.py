# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Factory for initializing environment-specific MoblyDriver implementations."""

import os
from typing import Any, Optional

from mobly_driver.api import api_infra
from mobly_driver.driver import base, common, infra, local


class DriverFactory:
    """Factory class for BaseDriver implementations.

    This class uses the runtime environment to determine the
    environment-specific MoblyDriver to use for driving a Mobly test. This
    allows for the same Python Mobly test binary/target to be defined once and
    run in multiple execution environments.
    """

    def __init__(
        self,
        honeydew_config: dict[str, Any],
        multi_device: bool = False,
        config_path: Optional[str] = None,
        params_path: Optional[str] = None,
        ssh_path: Optional[str] = None,
        target_address_type: Optional[str] = None,
        ap_ip: Optional[str] = None,
        ap_ssh_port: Optional[int] = None,
        ap_ssh_key: Optional[str] = None,
        ssh_key: Optional[str] = None,
    ) -> None:
        """Initializes the instance.
        Args:
          honeydew_config: Honeydew configuration.
          multi_device: whether the Mobly test requires 2+ devices to run.
          config_path: absolute path to the Mobly test config file.
          params_path: absolute path to the Mobly testbed params file.
          ffx_subtools_search_path: absolute path to where to search for FFX plugins.
          ssh_path: absolute path to the SSH binary.
          target_address_type: Whether to use the fuchsia device's name or ip for host-target
            interactions when using FFX and Fuchsia-Controller transports. Used only for local
            driver.
        """
        self._honeydew_config = honeydew_config
        self._multi_device = multi_device
        self._config_path = config_path
        self._params_path = params_path
        self._ssh_path = ssh_path
        self._target_address_type = target_address_type
        self._ap_ip = ap_ip
        self._ap_ssh_port = ap_ssh_port
        self._ap_ssh_key = ap_ssh_key
        self._ssh_key = ssh_key

    def get_driver(self) -> base.BaseDriver:
        """Returns an environment-specific Mobly Driver implementation.

        Returns:
          A base.BaseDriver implementation.

        Raises:
          common.DriverException if unexpected execution environment is found.
        """
        botanist_config_path = os.getenv(api_infra.BOT_ENV_TESTBED_CONFIG)
        if not botanist_config_path:
            return local.LocalDriver(
                honeydew_config=self._honeydew_config,
                multi_device=self._multi_device,
                config_path=self._config_path,
                params_path=self._params_path,
                target_address_type=self._target_address_type,
                ap_ip=self._ap_ip,
                ap_ssh_port=self._ap_ssh_port,
                ap_ssh_key=self._ap_ssh_key,
                ssh_key=self._ssh_key,
            )
        try:
            return infra.InfraDriver(
                tb_json_path=os.environ[api_infra.BOT_ENV_TESTBED_CONFIG],
                honeydew_config=self._honeydew_config,
                params_path=self._params_path,
                ssh_path=self._ssh_path,
            )
        except KeyError as e:
            raise common.DriverException(
                "Unexpected execution environment - missing env var: %s", e
            )
