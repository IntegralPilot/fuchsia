# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for Mobly driver's api_mobly.py."""

import json
import unittest
from tempfile import NamedTemporaryFile
from typing import Any

from mobly import config_parser, keys
from mobly_driver.api import api_mobly
from parameterized import param, parameterized

_HONEYDEW_CONFIG: dict[str, Any] = {
    "transports": {
        "ffx": {
            "path": "/ffx/path",
            "subtools_search_path": "subtools/search/path",
        }
    }
}


class ApiMoblyTest(unittest.TestCase):
    """API Mobly tests"""

    def test_get_latest_test_output_dir_symlink_path_success(self) -> None:
        """Test case to ensure test output symlink is returned"""
        api_mobly.get_latest_test_output_dir_symlink_path("output_path", "tb")

    @parameterized.expand(
        [
            ("Output path is empty", "", "tb"),
            ("Testbed name is empty", "ouput_path", ""),
        ]
    )
    def test_get_latest_test_output_dir_symlink_path_raises_exception(
        self, unused_name: str, output_path: str, tb_name: str
    ) -> None:
        """Test case to ensure exception is raised on test output path errors"""
        with self.assertRaises(api_mobly.ApiException):
            api_mobly.get_latest_test_output_dir_symlink_path(
                output_path, tb_name
            )

    def test_get_result_path_success(self) -> None:
        """Test case to ensure test result symlink is returned"""
        api_mobly.get_result_path("output_path", "tb")

    @parameterized.expand(
        [
            ("Output path is empty", "", "tb"),
            ("Testbed name is empty", "ouput_path", ""),
        ]
    )
    def test_get_get_result_path_raises_exception(
        self, unused_name: str, output_path: str, tb_name: str
    ) -> None:
        """Test case to ensure exception is raised on test result path errors"""
        with self.assertRaises(api_mobly.ApiException):
            api_mobly.get_result_path(output_path, tb_name)

    @parameterized.expand(
        [
            param(
                "success_empty_controllers_empty_params",
                override_args={
                    "mobly_controllers": [],
                    "test_params_dict": {},
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {"Controllers": {}, "Name": "tb_name", "TestParams": {}}
                    ],
                },
            ),
            param(
                "success_valid_controllers_valid_params",
                override_args={
                    "mobly_controllers": [
                        {"type": "FuchsiaDevice", "nodename": "fuchsia_abcd"}
                    ],
                    "test_params_dict": {"param": "value"},
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "FuchsiaDevice": [
                                    {
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                        "nodename": "fuchsia_abcd",
                                    }
                                ]
                            },
                            "Name": "tb_name",
                            "TestParams": {"param": "value"},
                        }
                    ],
                },
            ),
            param(
                "success_multiple_controllers_same_type",
                override_args={
                    "mobly_controllers": [
                        {"type": "FuchsiaDevice", "nodename": "fuchsia_abcd"},
                        {"type": "FuchsiaDevice", "nodename": "fuchsia_bcde"},
                    ]
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "FuchsiaDevice": [
                                    {
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                        "nodename": "fuchsia_abcd",
                                    },
                                    {
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                        "nodename": "fuchsia_bcde",
                                    },
                                ]
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_multiple_controllers_different_types",
                override_args={
                    "mobly_controllers": [
                        {"type": "FuchsiaDevice", "nodename": "fuchsia_abcd"},
                        {
                            "type": "AccessPoint",
                            "ip": "192.168.42.11",
                            "user": "root",
                            "ssh_key": "some/key/path",
                        },
                    ],
                    "ssh_path": "some/ssh_binary/path",
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "AccessPoint": [
                                    {
                                        "allow_regdb_bypass": False,
                                        "ssh_config": {
                                            "ssh_binary_path": "some/ssh_binary/path",
                                            "host": "192.168.42.11",
                                            "user": "root",
                                            "identity_file": "some/key/path",
                                        },
                                    },
                                ],
                                "FuchsiaDevice": [
                                    {
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                        "nodename": "fuchsia_abcd",
                                    }
                                ],
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_controller_with_translation_map",
                override_args={
                    "mobly_controllers": [
                        {
                            "type": "FuchsiaDevice",
                            "nodename": "fuchsia_abcd",
                        },
                    ],
                    "botanist_honeydew_map": {
                        "nodename": "name",
                    },
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "FuchsiaDevice": [
                                    {
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                        "name": "fuchsia_abcd",
                                    }
                                ]
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_access_point_ssh_config",
                override_args={
                    "mobly_controllers": [
                        {
                            "type": "AccessPoint",
                            "ip": "192.168.42.11",
                            "user": "root",
                            "ssh_key": "some/key/path",
                        },
                    ],
                    "ssh_path": "some/ssh_binary/path",
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "AccessPoint": [
                                    {
                                        "allow_regdb_bypass": False,
                                        "ssh_config": {
                                            "ssh_binary_path": "some/ssh_binary/path",
                                            "host": "192.168.42.11",
                                            "user": "root",
                                            "identity_file": "some/key/path",
                                        },
                                    },
                                ],
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_access_point_optional_fields",
                override_args={
                    "mobly_controllers": [
                        {
                            "type": "AccessPoint",
                            "ip": "192.168.42.11",
                            "user": "root",
                            "ssh_key": "some/key/path",
                            "nodename": "access-point-1234-5678-9abc",
                            "wan_interface": "eth0",
                            "pdu": {"ip": "192.168.42.23", "port": 1},
                        },
                    ],
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "AccessPoint": [
                                    {
                                        "allow_regdb_bypass": False,
                                        "nodename": "access-point-1234-5678-9abc",
                                        "wan_interface": "eth0",
                                        "pdu": {
                                            "ip": "192.168.42.23",
                                            "port": 1,
                                        },
                                        "ssh_config": {
                                            "host": "192.168.42.11",
                                            "user": "root",
                                            "identity_file": "some/key/path",
                                        },
                                    },
                                ],
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_access_point_openwrt_one",
                override_args={
                    "mobly_controllers": [
                        {
                            "type": "AccessPoint",
                            "ip": "192.168.42.11",
                            "user": "root",
                            "ssh_key": "some/key/path",
                            "model": "OpenWrtOne",
                        },
                    ],
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "OpenWrtAP": [
                                    {
                                        "allow_regdb_bypass": False,
                                        "ssh_config": {
                                            "host": "192.168.42.11",
                                            "user": "root",
                                            "identity_file": "some/key/path",
                                        },
                                    },
                                ],
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_access_point_ssh_config_no_ssh_path",
                override_args={
                    "mobly_controllers": [
                        {
                            "type": "AccessPoint",
                            "ip": "192.168.42.11",
                            "user": "root",
                            "ssh_key": "some/key/path",
                        },
                    ],
                    "ssh_path": None,
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "AccessPoint": [
                                    {
                                        "allow_regdb_bypass": False,
                                        "ssh_config": {
                                            "host": "192.168.42.11",
                                            "user": "root",
                                            "identity_file": "some/key/path",
                                        },
                                    },
                                ],
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
            param(
                "success_valid_controllers_monsoon",
                override_args={
                    "mobly_controllers": [
                        {
                            "type": "FuchsiaDevice",
                            "nodename": "fuchsia_abcd",
                            "monsoon": {"sernum": "123456"},
                        }
                    ],
                },
                expected_config_obj={
                    "MoblyParams": {"LogPath": "output_path"},
                    "TestBeds": [
                        {
                            "Controllers": {
                                "FuchsiaDevice": [
                                    {
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                        "nodename": "fuchsia_abcd",
                                        "monsoon": {
                                            "sernum": "123456",
                                        },
                                    }
                                ]
                            },
                            "Name": "tb_name",
                            "TestParams": {},
                        }
                    ],
                },
            ),
        ]
    )
    def test_new_testbed_config(
        self,
        unused_name: str,
        override_args: dict[str, Any],
        expected_config_obj: dict[str, Any],
    ) -> None:
        """Test case for new testbed config generation"""
        config_obj = api_mobly.new_testbed_config(
            testbed_name=override_args.get("testbed_name", "tb_name"),
            output_path=override_args.get("output_path", "output_path"),
            honeydew_config=_HONEYDEW_CONFIG,
            mobly_controllers=override_args.get("mobly_controllers", []),
            test_params_dict=override_args.get("test_params_dict", {}),
            botanist_honeydew_map=override_args.get(
                "botanist_honeydew_map", {}
            ),
            ssh_path=override_args.get("ssh_path", None),
        )
        self.assertEqual(config_obj, expected_config_obj)

        with NamedTemporaryFile(mode="w") as config_fh:
            config_fh.write(json.dumps(config_obj))
            config_fh.flush()
            # Assert that no exceptions are raised.
            config_parser.load_test_config_file(config_fh.name)

    @parameterized.expand(
        [
            ("success_empty_config", {}, {}),
            (
                "success_empty_controllers",
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {},
                        },
                    ],
                },
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {},
                        },
                    ]
                },
            ),
            (
                "success_valid_controllers",
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [{"nodename": "fuchsia_abcd"}]
                            },
                        }
                    ]
                },
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [
                                    {
                                        "nodename": "fuchsia_abcd",
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                    }
                                ]
                            },
                        }
                    ]
                },
            ),
            (
                "success_existing_value_is_overriden",
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [
                                    {
                                        "nodename": "fuchsia_abcd",
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                    }
                                ]
                            },
                        }
                    ]
                },
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [
                                    {
                                        "nodename": "fuchsia_abcd",
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                    }
                                ]
                            },
                        }
                    ]
                },
            ),
            (
                "success_multiple_controllers_same_type",
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [
                                    {"nodename": "fuchsia_abcd"},
                                    {"nodename": "fuchsia_bcde"},
                                ]
                            },
                        }
                    ]
                },
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [
                                    {
                                        "nodename": "fuchsia_abcd",
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                    },
                                    {
                                        "nodename": "fuchsia_bcde",
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                    },
                                ]
                            },
                        }
                    ]
                },
            ),
            (
                "success_multiple_controllers_different_types",
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [{"nodename": "fuchsia_abcd"}],
                                "AccessPoint": [{"ip_addr": "1.1.1.1"}],
                            },
                        }
                    ]
                },
                {
                    keys.Config.key_testbed.value: [
                        {
                            keys.Config.key_testbed_name.value: "testbed",
                            keys.Config.key_testbed_controllers.value: {
                                "FuchsiaDevice": [
                                    {
                                        "nodename": "fuchsia_abcd",
                                        "honeydew_config": _HONEYDEW_CONFIG,
                                    }
                                ],
                                "AccessPoint": [{"ip_addr": "1.1.1.1"}],
                            },
                        }
                    ]
                },
            ),
        ]
    )
    def test_set_honeydew_config(
        self,
        unused_name: str,
        config_obj: dict[str, Any],
        transformed_config_obj: dict[str, Any],
    ) -> None:
        """Test case for mutating honeydew_config in FuchsiaDevice config"""
        new_config_obj = config_obj.copy()
        api_mobly.set_honeydew_config(new_config_obj, _HONEYDEW_CONFIG)
        self.assertDictEqual(new_config_obj, transformed_config_obj)

    @parameterized.expand(
        [
            # (name, config_dict, params_dict, expected_config_dict)
            (
                "Single-testbed config with params",
                {"TestBeds": [{"Name": "tb_1"}]},
                {"param_1": "val_1"},
                {
                    "TestBeds": [
                        {"Name": "tb_1", "TestParams": {"param_1": "val_1"}}
                    ]
                },
            ),
            (
                "Multi-testbed config with params",
                {"TestBeds": [{"Name": "tb_1"}, {"Name": "tb_2"}]},
                {"param_1": "val_1"},
                {
                    "TestBeds": [
                        {"Name": "tb_1", "TestParams": {"param_1": "val_1"}},
                        {"Name": "tb_2", "TestParams": {"param_1": "val_1"}},
                    ]
                },
            ),
            (
                "Empty params",
                {"TestBeds": [{"Name": "tb_1"}]},
                {},
                {"TestBeds": [{"Name": "tb_1", "TestParams": {}}]},
            ),
            (
                "None params",
                {"TestBeds": [{"Name": "tb_1"}]},
                None,
                {"TestBeds": [{"Name": "tb_1", "TestParams": None}]},
            ),
        ]
    )
    def test_get_config_with_test_params_success(
        self,
        unused_name: str,
        config_dict: dict[str, Any],
        params_dict: dict[str, Any],
        expected_config_dict: dict[str, Any],
    ) -> None:
        """Test case for testbed config with params"""
        ret = api_mobly.get_config_with_test_params(config_dict, params_dict)
        self.assertDictEqual(ret, expected_config_dict)

    @parameterized.expand(
        [
            ("Config is None", None),
            ("Config is empty", {}),
        ]
    )
    def test_get_config_with_test_params_raises_exception(
        self, unused_name: str, config_dict: dict[str, Any]
    ) -> None:
        """Test case for exceptions in testbed config generation"""
        with self.assertRaises(api_mobly.ApiException):
            api_mobly.get_config_with_test_params(config_dict, {})
