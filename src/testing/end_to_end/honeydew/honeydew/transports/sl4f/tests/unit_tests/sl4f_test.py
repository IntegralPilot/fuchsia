# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for sl4f.py."""

import ipaddress
import unittest
from collections.abc import Callable
from typing import Any
from unittest import mock

from honeydew import affordances_capable, errors
from honeydew.transports.ffx import errors as ffx_errors
from honeydew.transports.ffx import ffx
from honeydew.transports.sl4f import errors as sl4f_errors
from honeydew.transports.sl4f import sl4f
from honeydew.typing import custom_types
from honeydew.utils import http_utils
from parameterized import param, parameterized

# pylint: disable=protected-access

_IPV4: str = "11.22.33.44"
_IPV4_OBJ: ipaddress.IPv4Address = ipaddress.IPv4Address(_IPV4)

_IPV6: str = "fe80::4fce:3102:ef13:888c%1"
_IPV6_OBJ: ipaddress.IPv6Address = ipaddress.IPv6Address(_IPV6)

_DEVICE_NAME: str = "fuchsia-emulator"

_IPV6_LOCALHOST: str = "::1"
_IPV6_LOCALHOST_OBJ: ipaddress.IPv6Address = ipaddress.IPv6Address(
    _IPV6_LOCALHOST
)

_SL4F_PORT_LOCAL: int = sl4f._SL4F_PORT["LOCAL"]
_SL4F_PORT_REMOTE: int = sl4f._SL4F_PORT["REMOTE"]
_SSH_PORT: int = 22

_INPUT_ARGS: dict[str, Any] = {
    "device_name": _DEVICE_NAME,
    "device_ip_v4": _IPV4_OBJ,
    "device_ip_v6": _IPV6_OBJ,
}

_MOCK_ARGS: dict[str, Any] = {
    "device_name": _DEVICE_NAME,
    "invalid-device_name": "invalid-device_name",
    "sl4f_server_address_ipv4": custom_types.Sl4fServerAddress(
        ip=_IPV4_OBJ, port=_SL4F_PORT_LOCAL
    ),
    "sl4f_server_address_ipv6": custom_types.Sl4fServerAddress(
        ip=_IPV6_OBJ, port=_SL4F_PORT_LOCAL
    ),
    "sl4f_server_address_ipv6_localhost": custom_types.Sl4fServerAddress(
        ip=_IPV6_LOCALHOST_OBJ, port=_SL4F_PORT_REMOTE
    ),
    "target_ssh_address_ipv4": custom_types.TargetSshAddress(
        ip=_IPV4_OBJ, port=_SSH_PORT
    ),
    "target_ssh_address_ipv6": custom_types.TargetSshAddress(
        ip=_IPV6_OBJ, port=_SSH_PORT
    ),
    "target_ssh_address_ipv6_localhost": custom_types.TargetSshAddress(
        ip=_IPV6_LOCALHOST_OBJ, port=_SSH_PORT
    ),
    "sl4f_request": sl4f._SL4F_METHODS["GetDeviceName"],
    "sl4f_response": {
        "id": "",
        "result": _DEVICE_NAME,
        "error": None,
    },
    "sl4f_error_response": {
        "id": "",
        "error": "some error",
    },
    "sl4f_url_v4": "http://{_IPV4}:{_SL4F_PORT_LOCAL}",
}

_EXPECTED_VALUES: dict[str, Any] = {
    "url_ipv4": f"http://{_IPV4}:{_SL4F_PORT_LOCAL}",
    "url_ipv6": f"http://[{_IPV6}]:{_SL4F_PORT_LOCAL}",
    "url_ipv6_localhost": f"http://[{_IPV6_LOCALHOST}]:{_SL4F_PORT_REMOTE}",
    "sl4f_server_address_ipv4": custom_types.Sl4fServerAddress(
        ip=_IPV4_OBJ, port=_SL4F_PORT_LOCAL
    ),
    "sl4f_server_address_ipv6": custom_types.Sl4fServerAddress(
        ip=_IPV6_OBJ, port=_SL4F_PORT_LOCAL
    ),
    "sl4f_server_address_ipv6_localhost": custom_types.Sl4fServerAddress(
        ip=_IPV6_LOCALHOST_OBJ, port=_SL4F_PORT_REMOTE
    ),
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom test name function method."""
    test_func_name: str = testcase_func.__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_with_{test_label}"


class SL4FTests(unittest.TestCase):
    """Unit tests for sl4f.py."""

    def setUp(self) -> None:
        super().setUp()

        self.ffx_obj = mock.MagicMock(spec=ffx.FFX)

        self.device_ip_change = mock.MagicMock(
            spec=affordances_capable.FuchsiaDeviceIpChange
        )

        with mock.patch.object(
            sl4f.SL4F, "start_server", autospec=True
        ) as mock_sl4f_start_server:
            self.sl4f_obj_wo_ip = sl4f.SL4F(
                device_name=_INPUT_ARGS["device_name"],
                ffx_transport=self.ffx_obj,
            )

            self.sl4f_obj_with_ipv4 = sl4f.SL4F(
                device_name=_INPUT_ARGS["device_name"],
                device_ip=_INPUT_ARGS["device_ip_v4"],
                ffx_transport=self.ffx_obj,
                device_ip_change=self.device_ip_change,
            )

            self.sl4f_obj_with_ipv6 = sl4f.SL4F(
                device_name=_INPUT_ARGS["device_name"],
                device_ip=_INPUT_ARGS["device_ip_v6"],
                ffx_transport=self.ffx_obj,
                device_ip_change=self.device_ip_change,
            )

            self.assertEqual(mock_sl4f_start_server.call_count, 3)

    @parameterized.expand(
        [
            (
                {
                    "label": "ipv4_address",
                    "sl4f_server_address": _MOCK_ARGS[
                        "sl4f_server_address_ipv4"
                    ],
                    "expected_url": _EXPECTED_VALUES["url_ipv4"],
                },
            ),
            (
                {
                    "label": "ipv6_address",
                    "sl4f_server_address": _MOCK_ARGS[
                        "sl4f_server_address_ipv6"
                    ],
                    "expected_url": _EXPECTED_VALUES["url_ipv6"],
                },
            ),
            (
                {
                    "label": "ipv6_localhost_address",
                    "sl4f_server_address": _MOCK_ARGS[
                        "sl4f_server_address_ipv6_localhost"
                    ],
                    "expected_url": _EXPECTED_VALUES["url_ipv6_localhost"],
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(sl4f.SL4F, "_get_sl4f_server_address", autospec=True)
    def test_sl4f_url(
        self,
        parameterized_dict: dict[str, Any],
        mock_get_sl4f_server_address: mock.Mock,
    ) -> None:
        """Testcase for SL4F.url property.

        It also tests SL4F._get_ip_version()."""
        mock_get_sl4f_server_address.return_value = parameterized_dict[
            "sl4f_server_address"
        ]

        self.assertEqual(
            self.sl4f_obj_wo_ip.url, parameterized_dict["expected_url"]
        )

        mock_get_sl4f_server_address.assert_called()

    @mock.patch.object(
        sl4f.SL4F,
        "run",
        return_value={"result": _MOCK_ARGS["device_name"]},
        autospec=True,
    )
    def test_check_connection(self, mock_sl4f_run: mock.Mock) -> None:
        """Testcase for SL4F.check_connection()"""
        self.sl4f_obj_wo_ip.check_connection()

        mock_sl4f_run.assert_called()

    @mock.patch.object(
        sl4f.SL4F,
        "run",
        return_value={"result": _MOCK_ARGS["invalid-device_name"]},
        autospec=True,
    )
    def test_check_connection_exception(self, mock_sl4f_run: mock.Mock) -> None:
        """Testcase for SL4F.check_connection() raising exception"""
        with self.assertRaises(sl4f_errors.Sl4fConnectionError):
            self.sl4f_obj_wo_ip.check_connection()

        mock_sl4f_run.assert_called()

    @parameterized.expand(
        [
            (
                {
                    "label": "just_mandatory_method_arg",
                    "method": _MOCK_ARGS["sl4f_request"],
                    "optional_params": {},
                    "mock_http_response": _MOCK_ARGS["sl4f_response"],
                },
            ),
            (
                {
                    "label": "optional_params_arg",
                    "method": _MOCK_ARGS["sl4f_request"],
                    "optional_params": {
                        "params": {
                            "message": "message",
                        },
                    },
                    "mock_http_response": _MOCK_ARGS["sl4f_response"],
                },
            ),
            (
                {
                    "label": "all_optional_params",
                    "method": _MOCK_ARGS["sl4f_request"],
                    "optional_params": {
                        "params": {
                            "message": "message",
                        },
                        "timeout": 3,
                        "attempts": 3,
                        "interval": 3,
                        "exceptions_to_skip": [],
                    },
                    "mock_http_response": _MOCK_ARGS["sl4f_response"],
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(
        http_utils,
        "send_http_request",
        return_value=_MOCK_ARGS["sl4f_response"],
        autospec=True,
    )
    @mock.patch.object(
        sl4f.SL4F,
        "url",
        new_callable=mock.PropertyMock,
        return_value=_MOCK_ARGS["sl4f_url_v4"],
    )
    def test_sl4f_run(
        self,
        parameterized_dict: dict[str, Any],
        mock_sl4f_url: mock.Mock,
        mock_send_http_request: mock.Mock,
    ) -> None:
        """Testcase for SL4F.run() success case"""
        method: str = parameterized_dict["method"]
        optional_params: dict[str, Any] = parameterized_dict["optional_params"]

        response: dict[str, Any] = self.sl4f_obj_wo_ip.run(
            method=method, **optional_params
        )

        self.assertEqual(response, parameterized_dict["mock_http_response"])

        mock_sl4f_url.assert_called()
        mock_send_http_request.assert_called()

    @mock.patch.object(
        http_utils,
        "send_http_request",
        return_value=_MOCK_ARGS["sl4f_error_response"],
        autospec=True,
    )
    @mock.patch.object(
        sl4f.SL4F,
        "url",
        new_callable=mock.PropertyMock,
        return_value=_MOCK_ARGS["sl4f_url_v4"],
    )
    def test_sl4f_run_fail_because_of_error_in_resp(
        self, mock_sl4f_url: mock.Mock, mock_send_http_request: mock.Mock
    ) -> None:
        """Testcase for SL4F.run() failure case when there is 'error' in SL4F
        response received"""
        with self.assertRaises(sl4f_errors.Sl4fError):
            self.sl4f_obj_wo_ip.run(
                method=_MOCK_ARGS["sl4f_request"], attempts=5, interval=0
            )

        mock_sl4f_url.assert_called()
        mock_send_http_request.assert_called()

    @mock.patch.object(
        http_utils,
        "send_http_request",
        side_effect=errors.HttpRequestError("some run time error"),
        autospec=True,
    )
    @mock.patch.object(
        sl4f.SL4F,
        "url",
        new_callable=mock.PropertyMock,
        return_value=_MOCK_ARGS["sl4f_url_v4"],
    )
    def test_send_sl4f_command_fail_because_of_exception(
        self, mock_sl4f_url: mock.Mock, mock_send_http_request: mock.Mock
    ) -> None:
        """Testcase for SL4F.run() failure case when there is an exception
        thrown while sending HTTP request"""
        with self.assertRaises(sl4f_errors.Sl4fError):
            self.sl4f_obj_wo_ip.run(
                method=_MOCK_ARGS["sl4f_request"], attempts=5, interval=0
            )

        mock_sl4f_url.assert_called()
        mock_send_http_request.assert_called_once()

    @mock.patch.object(
        http_utils,
        "send_http_request",
        side_effect=errors.HttpTimeoutError(""),
        autospec=True,
    )
    def test_send_sl4f_command_timeout(
        self, mock_send_http_request: mock.Mock
    ) -> None:
        """Verify SL4F.run() failure case for HTTP timeouts."""
        with self.assertRaises(TimeoutError):
            self.sl4f_obj_with_ipv4.run(
                method=_MOCK_ARGS["sl4f_request"], attempts=5, interval=0
            )

        mock_send_http_request.assert_called_once()

    @mock.patch.object(sl4f.SL4F, "check_connection", autospec=True)
    def test_start_server(self, mock_check_connection: mock.Mock) -> None:
        """Testcase for SL4F.start_server()"""
        self.sl4f_obj_wo_ip.start_server()

        mock_check_connection.assert_called()

    def test_start_server_exception(self) -> None:
        """Testcase for SL4F.start_server() raising exception"""
        self.ffx_obj.run_ssh_cmd.side_effect = ffx_errors.FfxCommandError(
            "error"
        )
        with self.assertRaises(sl4f_errors.Sl4fError):
            self.sl4f_obj_wo_ip.start_server()

    @parameterized.expand(
        [
            (
                {
                    "label": "ipv4",
                    "target_ssh_address": _MOCK_ARGS["target_ssh_address_ipv4"],
                    "expected_sl4f_address": _EXPECTED_VALUES[
                        "sl4f_server_address_ipv4"
                    ],
                },
            ),
            (
                {
                    "label": "ipv6",
                    "target_ssh_address": _MOCK_ARGS["target_ssh_address_ipv6"],
                    "expected_sl4f_address": _EXPECTED_VALUES[
                        "sl4f_server_address_ipv6"
                    ],
                },
            ),
            (
                {
                    "label": "ipv6_localhost",
                    "target_ssh_address": _MOCK_ARGS[
                        "target_ssh_address_ipv6_localhost"
                    ],
                    "expected_sl4f_address": _EXPECTED_VALUES[
                        "sl4f_server_address_ipv6_localhost"
                    ],
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    def test_get_sl4f_server_address_without_device_ip(
        self, parameterized_dict: dict[str, Any]
    ) -> None:
        """Testcase for SL4F._get_sl4f_server_address() when called using SL4F
        object created without device_ip argument."""
        self.ffx_obj.get_target_ssh_address.return_value = parameterized_dict[
            "target_ssh_address"
        ]

        self.assertEqual(
            self.sl4f_obj_wo_ip._get_sl4f_server_address(),
            parameterized_dict["expected_sl4f_address"],
        )

    def test_get_sl4f_server_address_with_device_ip(self) -> None:
        """Testcase for SL4F._get_sl4f_server_address() when called using SL4F
        object created with device_ip argument."""

        self.assertEqual(
            self.sl4f_obj_with_ipv4._get_sl4f_server_address(),
            _EXPECTED_VALUES["sl4f_server_address_ipv4"],
        )

        self.assertEqual(
            self.sl4f_obj_with_ipv6._get_sl4f_server_address(),
            _EXPECTED_VALUES["sl4f_server_address_ipv6"],
        )

    def test_get_sl4f_server_address_none_ssh_address(self) -> None:
        """Testcase for SL4F._get_sl4f_server_address() when ssh_addr is None."""
        self.ffx_obj.get_target_ssh_address.return_value = None

        with self.assertRaises(sl4f_errors.Sl4fError):
            self.sl4f_obj_wo_ip._get_sl4f_server_address()
