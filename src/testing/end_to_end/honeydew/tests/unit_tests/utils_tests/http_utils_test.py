# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for honeydew.utils.http_utils.py."""

import json
import time
import unittest
import urllib.error
import urllib.request
from collections.abc import Callable
from http.client import RemoteDisconnected
from typing import Any
from unittest import mock

from honeydew import errors
from honeydew.utils import http_utils
from parameterized import param, parameterized

# pylint: disable=protected-access
_PARAMS: dict[str, Any] = {
    "url": "http://12.34.56.78",
    "data": {
        "key1": "value1",
        "key2": "",
        "key3": {"k.3.1": "v.3.1"},
    },
    "headers": {
        "Content-Type": "application/json; charset=utf-8",
        "Content-Length": 58,  # len(json.dumps(_PARAMS["data"]))
    },
    "timeout": 15,
    "attempts": http_utils._DEFAULTS["ATTEMPTS"] + 5,
    "interval": http_utils._DEFAULTS["INTERVAL"] + 5,
    "exceptions_to_skip": [],
}

_MOCK_ARGS: dict[str, Any] = {
    "urlopen_resp": b'{"id": "", "result": "fuchsia-emulator", "error": null}'
}


def _custom_test_name_func(
    testcase_func: Callable[..., None], _: str, param_arg: param
) -> str:
    """Custom name function method."""
    test_func_name: str = testcase_func.__name__

    params_dict: dict[str, Any] = param_arg.args[0]
    test_label: str = parameterized.to_safe_name(params_dict["label"])

    return f"{test_func_name}_with_{test_label}"


class HttpUtilsTests(unittest.TestCase):
    """Unit tests for honeydew.utils.http_utils.py."""

    @parameterized.expand(
        [
            (
                {
                    "label": "no_optional_params",
                    "url": _PARAMS["url"],
                    "optional_params": {},
                    "urlopen_resp": _MOCK_ARGS["urlopen_resp"],
                },
            ),
            (
                {
                    "label": "data_and_headers",
                    "url": _PARAMS["url"],
                    "optional_params": {
                        "data": _PARAMS["data"],
                        "headers": _PARAMS["headers"],
                    },
                    "urlopen_resp": _MOCK_ARGS["urlopen_resp"],
                },
            ),
            (
                {
                    "label": "data_but_no_headers",
                    "url": _PARAMS["url"],
                    "optional_params": {
                        "data": _PARAMS["data"],
                    },
                    "urlopen_resp": _MOCK_ARGS["urlopen_resp"],
                },
            ),
            (
                {
                    "label": "no_data",
                    "url": _PARAMS["url"],
                    "optional_params": {},
                    "urlopen_resp": _MOCK_ARGS["urlopen_resp"],
                },
            ),
            (
                {
                    "label": "skip_ssl_cert_check",
                    "url": _PARAMS["url"],
                    "optional_params": {
                        "verify_ssl": False,
                    },
                    "urlopen_resp": _MOCK_ARGS["urlopen_resp"],
                },
            ),
            (
                {
                    "label": "all_optional_params",
                    "url": _PARAMS["url"],
                    "optional_params": {
                        "data": _PARAMS["data"],
                        "headers": _PARAMS["headers"],
                        "timeout": _PARAMS["timeout"],
                        "attempts": _PARAMS["attempts"],
                        "interval": _PARAMS["interval"],
                        "exceptions_to_skip": _PARAMS["exceptions_to_skip"],
                        "verify_ssl": False,
                    },
                    "urlopen_resp": _MOCK_ARGS["urlopen_resp"],
                },
            ),
        ],
        name_func=_custom_test_name_func,
    )
    @mock.patch.object(urllib.request, "urlopen", autospec=True)
    def test_send_http_request_success(
        self, parameterized_dict: dict[str, Any], mock_urlopen: mock.Mock
    ) -> None:
        """Test case for http_utils.send_http_request() success case."""

        urlopen_return_value = mock.MagicMock()
        urlopen_return_value.read.return_value = parameterized_dict[
            "urlopen_resp"
        ]
        urlopen_return_value.__enter__.return_value = urlopen_return_value
        mock_urlopen.return_value = urlopen_return_value

        result: dict[str, Any] = http_utils.send_http_request(
            url=parameterized_dict["url"],
            **parameterized_dict["optional_params"],
        )

        expected_output: dict[str, Any] = json.loads(
            parameterized_dict["urlopen_resp"].decode("utf-8")
        )

        self.assertEqual(result, expected_output)

        mock_urlopen.assert_called_once()

    @mock.patch.object(
        urllib.request,
        "urlopen",
        side_effect=RemoteDisconnected,
        autospec=True,
    )
    def test_send_http_request_with_exceptions_to_skip(
        self, mock_urlopen: mock.Mock
    ) -> None:
        """Testcase to make sure http_utils.send_http_request() do not
        fail when it receives an exception that is part of exceptions_to_skip
        input arg"""
        response: dict[str, Any] = http_utils.send_http_request(
            url=_PARAMS["url"], exceptions_to_skip=[RemoteDisconnected]
        )
        self.assertEqual(response, {})
        mock_urlopen.assert_called_once()

    @mock.patch.object(time, "sleep", autospec=True)
    @mock.patch.object(
        urllib.request,
        "urlopen",
        side_effect=ConnectionError("some run time error"),
        autospec=True,
    )
    def test_send_http_request_fail_because_of_exception(
        self, mock_urlopen: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Testcase for http_utils.send_http_request() failure case because of
        an exception."""
        with self.assertRaises(errors.HttpRequestError):
            http_utils.send_http_request(
                url=_PARAMS["url"],
                interval=_PARAMS["interval"],
                attempts=_PARAMS["attempts"],
            )

        self.assertEqual(mock_urlopen.call_count, _PARAMS["attempts"])
        self.assertEqual(mock_sleep.call_count, _PARAMS["attempts"] - 1)

    @mock.patch.object(time, "sleep", autospec=True)
    @mock.patch.object(
        urllib.request,
        "urlopen",
        side_effect=urllib.error.URLError(TimeoutError()),
        autospec=True,
    )
    def test_timeout_error(
        self, mock_urlopen: mock.Mock, mock_sleep: mock.Mock
    ) -> None:
        """Verify http_utils.send_http_request() can raise TimeoutError."""
        with self.assertRaises(TimeoutError):
            http_utils.send_http_request(
                url=_PARAMS["url"],
                interval=_PARAMS["interval"],
                attempts=_PARAMS["attempts"],
            )

        self.assertEqual(mock_urlopen.call_count, _PARAMS["attempts"])
        self.assertEqual(mock_sleep.call_count, _PARAMS["attempts"] - 1)


if __name__ == "__main__":
    unittest.main()
