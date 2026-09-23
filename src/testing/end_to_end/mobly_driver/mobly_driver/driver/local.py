# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Implements BaseDriver for the local execution environment."""

import os
import time
from copy import deepcopy
from typing import Any, Dict, List, Optional

import yaml
from mobly import keys
from mobly_driver.api import api_ffx, api_infra, api_mobly
from mobly_driver.driver import base, common

_VALID_TARGET_TYPES: list[str] = [
    "ip",
    "id",
    "name",
]


class LocalDriver(base.BaseDriver):
    """Local Mobly test driver.

    This driver is used when executing Mobly tests in the local environment.
    In the local environment, it is assumed that users have full knowledge of
    the physical testbed that will be used during the Mobly test so LocalDriver
    allows for the Mobly |config_path| to be supplied directly by the user.
    """

    def __init__(
        self,
        honeydew_config: dict[str, Any],
        multi_device: bool = False,
        output_path: Optional[str] = None,
        config_path: Optional[str] = None,
        params_path: Optional[str] = None,
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
          output_path: absolute path to directory for storing Mobly test output.
          config_path: absolute path to the Mobly test config file.
          params_path: absolute path to the Mobly test params file.
          target_address_type: Whether to use the fuchsia device's name, serial
            number,  or ip for host-target interactions when using FFX and
            Fuchsia-Controller transports.

        Raises:
          KeyError if required environment variables not found.
          ValueError if incorrect value is passed for any of the init argument.
        """
        super().__init__(
            honeydew_config=honeydew_config,
            output_path=output_path,
            params_path=params_path,
        )
        self._multi_device = multi_device
        self._config_path = config_path

        ffx_config = honeydew_config.get("transports", {}).get("ffx", {})
        ssh_auth_sock = ffx_config.get("ssh_auth_sock") or ffx_config.get(
            "ssh.auth-sock"
        )
        self._ffx_client = api_ffx.FfxClient(
            ffx_path=ffx_config.get("path", ""),
            ssh_auth_sock=ssh_auth_sock,
            identities_only=ffx_config.get("identities_only"),
        )
        self._ap_ip = ap_ip
        self._ap_ssh_port = ap_ssh_port
        self._ap_ssh_key = ap_ssh_key
        self._ssh_key = ssh_key

        self._target_address_type: Optional[str] = target_address_type

        if (
            self._target_address_type
            and self._target_address_type not in _VALID_TARGET_TYPES
        ):
            raise ValueError(
                f"'target_address_type' should be from '{_VALID_TARGET_TYPES}' but received: "
                f"{self._target_address_type}"
            )

    def _get_test_targets(self) -> List[str]:
        """Returns Fuchsia target names to use in Mobly test.

        * If multi-device test, return all discovered target(s).
        * If single-device test and default device is not set, return all
          discovered target(s).
        * If single-device test and default device is set, return only default
          target(s).

        Returns:
          A list of Fuchsia target names.

        Raises:
          common.DriverException if device discovery command fails or no devices
            detected.
        """
        last_error = None
        res = None
        for i in range(10):
            try:
                res = self._ffx_client.target_list(
                    # Run without isolate dir to access relevant "default" device.
                    isolate_dir=None
                )
                if len(res.all_nodes) > 0:
                    break
            except (
                api_ffx.CommandException,
                api_ffx.OutputFormatException,
            ) as e:
                # If it fails, maybe daemon is starting, so retry
                last_error = e
            print(
                f"No targets discovered yet, retrying in 2s... (attempt {i+1}/10)"
            )
            time.sleep(2)

        if not res:
            if last_error:
                raise common.DriverException(
                    f"Failed to enumerate local targets: {last_error}"
                )
            raise common.DriverException("Failed to enumerate local targets")

        test_targets: List[str] = res.all_nodes
        if self._multi_device:
            print(f"Multi-device: test with all discovered target(s).")
        elif not res.default_nodes:
            print(f"No default target set: test with all discovered target(s).")
        else:
            print(f"Default target set: test with default target(s).")
            test_targets = res.default_nodes

        if len(test_targets) == 0:
            # Raise exception here because any meaningful Mobly test should run
            # against at least one Fuchsia target.
            raise common.DriverException("No devices found after retries.")

        print(f"Target(s) to use in Mobly test: {test_targets}")
        return test_targets

    def _generate_config_from_env(self) -> api_mobly.MoblyConfigComponent:
        """Returns Mobly device config generated from local environment.

        Best effort config generation based on Fuchsia device discovery on local
        host.

        Returns:
          A list of Fuchsia target names.

        Raises:
          common.InvalidFormatException if unable to extract target names from
            device discovery output.
          common.DriverException if device discovery command fails or no devices
            detected.
        """
        mobly_controllers: List[Dict[str, Any]] = []
        honeydew_config = deepcopy(self._honeydew_config)

        usb_socket_path = os.getenv("FUCHSIA_TEST_FFX_USB_SOCKET_PATH")
        if usb_socket_path:
            honeydew_config["transports"]["ffx"][
                "usb_socket_path"
            ] = usb_socket_path
        for target in self._get_test_targets():
            fx_device = {
                "type": api_infra.FUCHSIA_DEVICE,
                "name": target,
            }
            if self._ssh_key:
                fx_device["ssh_key"] = self._ssh_key
            if (
                self._target_address_type == "id"
                or not self._target_address_type
            ):
                failure = None
                target_serial: str | None = None
                try:
                    target_serial = self._ffx_client.get_target_serial(
                        target_name=target, isolate_dir=None
                    )
                except Exception as e:
                    failure = e
                if target_serial is not None:
                    fx_device["device_serial"] = target_serial
                elif self._target_address_type == "id":
                    if failure is None:
                        raise common.DriverException(
                            "Device had no serial number (which is required when target_address_type is 'id')"
                        )
                    else:
                        raise failure

            if (
                self._target_address_type == "ip"
                or not self._target_address_type
            ):
                try:
                    target_ssh_address: api_ffx.TargetSshAddress = (
                        self._ffx_client.get_target_ssh_address(
                            target_name=target, isolate_dir=None
                        )
                    )
                    fx_device["device_ip_port"] = str(target_ssh_address)
                except Exception as e:
                    if (
                        self._target_address_type == "ip"
                        or "device_serial" not in fx_device
                    ):
                        raise e

            mobly_controllers.append(fx_device)

        if self._ap_ip:
            ap_config = {
                "type": api_infra.ACCESS_POINT,
                "ip": self._ap_ip,
                # Default SSH port 22 if not provided
                "port": self._ap_ssh_port or 22,
                "user": "root",
                "wan_interface": "eth0",
                "allow_regdb_bypass": False,
            }

            if self._ap_ssh_key:
                ap_config["ssh_key"] = self._ap_ssh_key
            else:
                # Try to find key in default locations
                home = os.path.expanduser("~")
                default_keys = [
                    os.path.join(home, ".ssh", "onhub_testing_rsa"),
                    os.path.join(home, ".ssh", "testing_rsa"),
                ]
                found_key = None
                for key_path in default_keys:
                    if os.path.exists(key_path):
                        found_key = key_path
                        break

                if found_key:
                    print(f"Using AP SSH key found at {found_key}")
                    ap_config["ssh_key"] = found_key
                else:
                    raise common.DriverException(
                        "AP IP provided but no SSH key found in default locations "
                        f"({default_keys}) and --ap-ssh-key not specified."
                    )

            mobly_controllers.append(ap_config)

        config = api_mobly.new_testbed_config(
            testbed_name="GeneratedLocalTestbed",
            output_path=self._output_path,
            honeydew_config=honeydew_config,
            mobly_controllers=mobly_controllers,
            test_params_dict={},
            botanist_honeydew_map={},
        )
        return config

    def generate_test_config(self) -> str:
        """Returns a Mobly test config in YAML format.

        The Mobly test config is a required input file of any Mobly tests.
        It includes information on the DUT(s) and specifies test parameters.

        Example output:
        ---
        TestBeds:
        - Name: SomeName
          Controllers:
            FuchsiaDevice:
            - name: fuchsia-1234-5678-90ab
          TestParams:
            param_1: "val_1"
            param_2: "val_2"

        If |params_path| is specified in LocalDriver(), then its content is
        added to the Mobly test config; otherwise, the test config is returned
        as-is but in YAML form.

        Returns:
          A YAML string that represents a Mobly test config.

        Raises:
          common.InvalidFormatException if the test params or tb config files
            are not valid YAML documents.
          common.DriverException if Mobly config generation fails.
        """
        config: Dict[str, Any] = {}
        if self._config_path is None:
            print("Generating Mobly config from environment...")
            print("(To override, provide path to YAML via `config_yaml_path`)")
            try:
                config = self._generate_config_from_env()
            except (
                common.DriverException,
                common.InvalidFormatException,
            ) as e:
                raise common.DriverException(
                    f"Local config generation failed: {e}"
                )
        else:
            print("Using provided Mobly config YAML...")
            try:
                config = common.read_yaml_from_file(self._config_path)
            except (IOError, OSError) as e:
                raise common.DriverException(f"Local config parse failed: {e}")
            # Add the "honeydew_config" field for every Fuchsia device, if exists.
            api_mobly.set_honeydew_config(config, self._honeydew_config)

        test_params = {}
        if self._params_path:
            test_params = common.read_yaml_from_file(self._params_path)

        if "transports" in self._honeydew_config:
            ffx_config = self._honeydew_config["transports"].get("ffx", {})
            if "subtools_search_path" in ffx_config:
                subtools_path = ffx_config["subtools_search_path"]
                if self._params_path:
                    test_params["ffx-subtools-search-path"] = subtools_path
                    config = api_mobly.get_config_with_test_params(
                        config, test_params
                    )
                else:
                    for tb in config.get(keys.Config.key_testbed.value, []):
                        tb_params = tb.setdefault(
                            keys.Config.key_testbed_test_params.value, {}
                        )
                        tb_params["ffx-subtools-search-path"] = subtools_path
        elif self._params_path:
            config = api_mobly.get_config_with_test_params(config, test_params)

        return yaml.dump(config)

    def teardown(self, *args: Any) -> None:
        pass
