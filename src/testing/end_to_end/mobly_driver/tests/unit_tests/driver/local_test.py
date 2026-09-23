# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for mobly_driver/driver/local.py."""

import ipaddress
import unittest
from collections.abc import Callable
from typing import Any
from unittest.mock import patch

from mobly_driver.api import api_ffx
from mobly_driver.driver import common, local
from parameterized import param, parameterized

_HONEYDEW_CONFIG: dict[str, Any] = {
    "transports": {
        "ffx": {
            "path": "/ffx/path",
            "subtools_search_path": "subtools/search/path",
        }
    }
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = testcase_func.__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_with_{test_label}"


class LocalDriverTest(unittest.TestCase):
    """Local Driver tests"""

    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch("mobly_driver.driver.common.read_yaml_from_file")
    @patch("mobly_driver.api.api_mobly.get_config_with_test_params")
    def test_generate_test_config_from_file_with_params_success(
        self, mock_get_config: Any, mock_read_yaml: Any, *unused_args: Any
    ) -> None:
        """Test case for successful config generation from file"""
        mock_read_yaml.side_effect = [
            {"TestBeds": [{"Name": "GeneratedLocalTestbed"}]},
            {"existing_param": "value"},
        ]
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            config_path="config/path",
            params_path="params/path",
        )
        ret = driver.generate_test_config()

        mock_get_config.assert_called_once_with(
            {"TestBeds": [{"Name": "GeneratedLocalTestbed"}]},
            {
                "existing_param": "value",
                "ffx-subtools-search-path": "subtools/search/path",
            },
        )
        self.assertEqual(mock_read_yaml.call_count, 2)
        self.assertEqual(ret, "yaml_str")

    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch("mobly_driver.driver.common.read_yaml_from_file")
    @patch("mobly_driver.api.api_mobly.get_config_with_test_params")
    def test_generate_test_config_from_file_without_params_success(
        self,
        mock_get_config: Any,
        mock_read_yaml: Any,
        mock_yaml_dump: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for successful config without params generation"""
        mock_read_yaml.return_value = {
            "TestBeds": [{"Name": "GeneratedLocalTestbed"}]
        }
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            config_path="config/path",
        )
        ret = driver.generate_test_config()

        mock_get_config.assert_not_called()
        mock_read_yaml.assert_called_once_with("config/path")
        mock_yaml_dump.assert_called_once_with(
            {
                "TestBeds": [
                    {
                        "Name": "GeneratedLocalTestbed",
                        "TestParams": {
                            "ffx-subtools-search-path": "subtools/search/path",
                        },
                    }
                ]
            }
        )
        self.assertEqual(ret, "yaml_str")

    @patch("builtins.print")
    @patch(
        "mobly_driver.driver.common.read_yaml_from_file",
        side_effect=common.InvalidFormatException,
    )
    def test_generate_test_config_from_file_invalid_yaml_content_raises_exception(
        self, *unused_args: Any
    ) -> None:
        """Test case for exception being raised on invalid YAML content"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            config_path="config/path",
        )
        with self.assertRaises(common.InvalidFormatException):
            driver.generate_test_config()

    @patch("builtins.print")
    @patch(
        "mobly_driver.driver.common.read_yaml_from_file", side_effect=OSError
    )
    def test_generate_test_config_from_file_invalid_path_raises_exception(
        self, *unused_args: Any
    ) -> None:
        """Test case for exception being raised for invalid path"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            config_path="/does/not/exist",
        )
        with self.assertRaises(common.DriverException):
            driver.generate_test_config()

    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=["dut_1", "dut_2"], default_nodes=[]
        ),
    )
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_generate_test_config_from_env_success(
        self,
        mock_new_tb_config: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        mock_yaml_dump: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for successful env config generation"""
        mock_new_tb_config.return_value = {
            "TestBeds": [
                {
                    "Name": "GeneratedLocalTestbed",
                    "Controllers": {},
                }
            ]
        }
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
        )
        ret = driver.generate_test_config()

        mock_new_tb_config.assert_called_once()
        controllers = mock_new_tb_config.call_args.kwargs["mobly_controllers"]
        self.assertEqual(2, len(controllers))
        self.assertEqual([c["name"] for c in controllers], ["dut_1", "dut_2"])

        mock_yaml_dump.assert_called_once_with(
            {
                "TestBeds": [
                    {
                        "Name": "GeneratedLocalTestbed",
                        "Controllers": {},
                        "TestParams": {
                            "ffx-subtools-search-path": "subtools/search/path",
                        },
                    }
                ]
            }
        )
        self.assertEqual(ret, "yaml_str")

        mock_ffx_target_list.assert_called()
        mock_ffx_target_ssh_address.assert_called()

    @parameterized.expand(
        [
            (
                {
                    "label": "not_passed",
                    "target_address_type": None,
                },
            ),
            (
                {
                    "label": "valid_value_as_ip_address",
                    "target_address_type": "ip",
                },
            ),
            (
                {
                    "label": "valid_value_as_name",
                    "target_address_type": "name",
                },
            ),
            (
                {
                    "label": "valid_value_as_id",
                    "target_address_type": "id",
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=["dut_1"], default_nodes=[]
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_serial",
        autospec=True,
        return_value="1234",
    )
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_target_address_type_arg_success(
        self,
        parameterized_dict: dict[str, Any],
        mock_new_tb_config: Any,
        mock_get_target_serial: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for target_address_type argument's success case"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            target_address_type=parameterized_dict["target_address_type"],
        )
        ret = driver.generate_test_config()

        mock_new_tb_config.assert_called_once()
        controllers = mock_new_tb_config.call_args.kwargs["mobly_controllers"]
        self.assertEqual(1, len(controllers))
        self.assertEqual([c["name"] for c in controllers], ["dut_1"])
        self.assertEqual(ret, "yaml_str")

        mock_ffx_target_list.assert_called()
        if parameterized_dict["target_address_type"] in [None, "id"]:
            mock_get_target_serial.assert_called_once()
        else:
            mock_get_target_serial.assert_not_called()

        if parameterized_dict["target_address_type"] in [None, "ip"]:
            mock_ffx_target_ssh_address.assert_called_once()
        else:
            mock_ffx_target_ssh_address.assert_not_called()

    def test_target_address_type_arg_exception(
        self,
    ) -> None:
        """Test case for target_address_type argument's failure case"""
        with self.assertRaises(ValueError):
            local.LocalDriver(
                honeydew_config=_HONEYDEW_CONFIG,
                output_path="output/path",
                target_address_type="invalid",
            )

    @parameterized.expand(
        [
            (
                "default_nodes exist, prefer all_nodes",
                ["dut_1"],
            ),
            ("default_nodes empty, prefer all_nodes", []),
        ]
    )
    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch("mobly_driver.api.api_ffx.FfxClient.target_list", autospec=True)
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_multi_device_config_generation(
        self,
        unused_name: str,
        default_nodes: list[str],
        mock_new_tb_config: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for multi-device config generation."""
        mock_ffx_target_list.return_value = api_ffx.TargetListResult(
            all_nodes=["dut_1", "dut_2"], default_nodes=default_nodes
        )

        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            multi_device=True,
        )
        ret = driver.generate_test_config()

        mock_new_tb_config.assert_called()
        controllers = mock_new_tb_config.call_args.kwargs["mobly_controllers"]
        self.assertEqual([c["name"] for c in controllers], ["dut_1", "dut_2"])
        self.assertEqual(ret, "yaml_str")

        mock_ffx_target_list.assert_called()
        mock_ffx_target_ssh_address.assert_called()

    @parameterized.expand(
        [
            ("default_nodes exist, prefer default_nodes", ["dut_1"], ["dut_1"]),
            ("default_nodes empty, prefer all_nodes", [], ["dut_1", "dut_2"]),
        ]
    )
    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch("mobly_driver.api.api_ffx.FfxClient.target_list", autospec=True)
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_single_device_config_generation(
        self,
        unused_name: str,
        default_nodes: list[str],
        want_nodes: list[str],
        mock_new_tb_config: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for single-device config generation."""
        mock_ffx_target_list.return_value = api_ffx.TargetListResult(
            all_nodes=["dut_1", "dut_2"], default_nodes=default_nodes
        )

        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            multi_device=False,
        )
        ret = driver.generate_test_config()

        mock_new_tb_config.assert_called()
        controllers = mock_new_tb_config.call_args.kwargs["mobly_controllers"]
        self.assertEqual([c["name"] for c in controllers], want_nodes)
        self.assertEqual(ret, "yaml_str")

        mock_ffx_target_list.assert_called()
        mock_ffx_target_ssh_address.assert_called()

    @patch("builtins.print")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=[],
            default_nodes=[],
        ),
    )
    def test_config_generation_no_devices_raises_exception(
        self, mock_check_output: Any, *unused_args: Any
    ) -> None:
        """Test case for exception being raised when no devices are found"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
        )
        with self.assertRaises(common.DriverException):
            driver.generate_test_config()

    @patch("builtins.print")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        side_effect=api_ffx.CommandException(),
        autospec=True,
    )
    def test_generate_test_config_from_env_discovery_failure_raises_exception(
        self, mock_check_output: Any, *unused_args: Any
    ) -> None:
        """Test case for exception being raised from discovery failure"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
        )
        with self.assertRaises(common.DriverException):
            driver.generate_test_config()

    @parameterized.expand(
        [
            ("Invalid JSON str", b""),
            ("No devices JSON str", b"[]"),
            ("Empty device JSON str", b"[{}]"),
        ]
    )
    @patch("builtins.print")
    @patch("subprocess.check_output", autospec=True)
    def test_generate_test_config_from_env_discovery_output_raises_exception(
        self,
        unused_name: str,
        discovery_output: bytes,
        mock_check_output: Any,
        unused_print: Any,
    ) -> None:
        """Test case for exception being raised from invalid discovery output"""
        mock_check_output.return_value = discovery_output
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
        )
        with self.assertRaises(common.DriverException):
            driver.generate_test_config()

    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=["dut_1"], default_nodes=[]
        ),
    )
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_generate_test_config_with_ap_ip_success(
        self,
        mock_new_tb_config: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for successful config generation with AP IP"""
        with patch("os.path.exists", return_value=True):
            driver = local.LocalDriver(
                honeydew_config=_HONEYDEW_CONFIG,
                output_path="output/path",
                ap_ip="192.168.1.1",
            )
            ret = driver.generate_test_config()

            mock_new_tb_config.assert_called_once()
            controllers = mock_new_tb_config.call_args.kwargs[
                "mobly_controllers"
            ]
            self.assertEqual(2, len(controllers))
            self.assertEqual(
                [c.get("name") or c.get("ip") for c in controllers],
                ["dut_1", "192.168.1.1"],
            )
            self.assertEqual(ret, "yaml_str")

    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=["dut_1"], default_nodes=[]
        ),
    )
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_generate_test_config_with_ap_full_success(
        self,
        mock_new_tb_config: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for successful config generation with full AP args"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            ap_ip="192.168.1.1",
            ap_ssh_port=2222,
            ap_ssh_key="/path/to/key",
        )
        driver.generate_test_config()

        mock_new_tb_config.assert_called_once()
        controllers = mock_new_tb_config.call_args.kwargs["mobly_controllers"]
        self.assertEqual(2, len(controllers))
        ap_config = controllers[1]
        self.assertEqual(ap_config["ip"], "192.168.1.1")
        self.assertEqual(ap_config["port"], 2222)
        self.assertEqual(ap_config["ssh_key"], "/path/to/key")

    @patch("builtins.print")
    @patch("yaml.dump", return_value="yaml_str")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=["dut_1"], default_nodes=[]
        ),
    )
    @patch("mobly_driver.api.api_mobly.new_testbed_config", autospec=True)
    def test_generate_test_config_with_ssh_key_success(
        self,
        mock_new_tb_config: Any,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for successful config generation with Fuchsia ssh_key"""
        driver = local.LocalDriver(
            honeydew_config=_HONEYDEW_CONFIG,
            output_path="output/path",
            ssh_key="/path/to/fuchsia/key",
        )
        driver.generate_test_config()

        mock_new_tb_config.assert_called_once()
        controllers = mock_new_tb_config.call_args.kwargs["mobly_controllers"]
        self.assertEqual(1, len(controllers))
        fx_config = controllers[0]
        self.assertEqual(fx_config["name"], "dut_1")
        self.assertEqual(fx_config["ssh_key"], "/path/to/fuchsia/key")

    @patch("builtins.print")
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.get_target_ssh_address",
        autospec=True,
        return_value=api_ffx.TargetSshAddress(
            ip=ipaddress.ip_address("::1"), port=8022
        ),
    )
    @patch(
        "mobly_driver.api.api_ffx.FfxClient.target_list",
        autospec=True,
        return_value=api_ffx.TargetListResult(
            all_nodes=["dut_1"], default_nodes=[]
        ),
    )
    def test_generate_test_config_with_ap_ip_no_key_raises_exception(
        self,
        mock_ffx_target_list: Any,
        mock_ffx_target_ssh_address: Any,
        *unused_args: Any,
    ) -> None:
        """Test case for exception when AP key is missing"""
        with patch("os.path.exists", return_value=False):
            driver = local.LocalDriver(
                honeydew_config=_HONEYDEW_CONFIG,
                output_path="output/path",
                ap_ip="192.168.1.1",
            )
            with self.assertRaises(common.DriverException):
                driver.generate_test_config()
