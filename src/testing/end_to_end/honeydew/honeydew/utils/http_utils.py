# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Utility module for HTTP requests."""

import json
import logging
import ssl
import time
import urllib.error
import urllib.request
from collections.abc import Iterable
from typing import Any

from honeydew import errors
from honeydew.utils import decorators

_LOGGER: logging.Logger = logging.getLogger(__name__)

_DEFAULTS: dict[str, int] = {
    "ATTEMPTS": 3,
    "INTERVAL": 1,
}


@decorators.liveness_check
def send_http_request(
    url: str,
    data: dict[str, Any] | None = None,
    headers: dict[str, Any] | None = None,
    timeout: float | None = None,
    attempts: int = _DEFAULTS["ATTEMPTS"],
    interval: int = _DEFAULTS["INTERVAL"],
    exceptions_to_skip: Iterable[type[Exception]] | None = None,
    verify_ssl: bool = True,
) -> dict[str, Any]:
    """Send HTTP request and returns the string response.

    This method encodes data dict arg into utf-8 bytes and sets "Content-Type"
    in headers dict arg to "application/json; charset=utf-8".

    Args:
        url: URL to which HTTP request need to be sent.
        data: data that needs to be set in HTTP request.
        headers: headers that need to be included while sending HTTP request.
        timeout: how long in sec to wait for HTTP connection attempt. By
            default, timeout is not set.
        attempts: number of attempts to try in case of a failure.
        interval: wait time in sec before each retry in case of a failure.
        exceptions_to_skip: Any non fatal HTTP exceptions for which retry will
            not be attempted.
        verify_ssl: Whether to verify the SSL certificate of the server.
    Returns:
        Returns the HTTP response received after converting into a dict.

    Raises:
        errors.HttpRequestError: In case of failures.
        errors.HttpTimeoutError: Requests timed out.
    """
    if exceptions_to_skip is None:
        exceptions_to_skip = []

    if headers is None:
        headers = {
            "Content-Type": "application/json; charset=utf-8",
        }

    data_bytes: bytes | None = None
    if data is not None:
        data_bytes = json.dumps(data).encode("utf-8")
        headers["Content-Length"] = len(data_bytes)

    context: ssl.SSLContext | None = None
    if not verify_ssl:
        # pylint: disable-next=protected-access
        context = ssl._create_unverified_context()

    for attempt in range(1, attempts + 1):
        # if this is not first attempt wait for sometime before next retry.
        if attempt > 1:
            time.sleep(interval)

        try:
            _LOGGER.debug(
                "Sending HTTP request to url=%s with data=%s and headers=%s",
                url,
                data,
                headers,
            )
            req = urllib.request.Request(url, data=data_bytes, headers=headers)
            with urllib.request.urlopen(
                req, timeout=timeout, context=context
            ) as response:
                response_body: str = response.read().decode("utf-8")
            _LOGGER.debug(
                "HTTP response received from url=%s is '%s'", url, response_body
            )
            return json.loads(response_body)
        except (
            ConnectionError,
            urllib.error.URLError,
        ) as err:
            for exception_to_skip in exceptions_to_skip:
                if isinstance(err, exception_to_skip):
                    return {}

            err_msg: str = (
                f"Send the HTTP request to url={url} with "
                f"data={data} and headers={headers} failed with error: '{err}'"
            )

            if attempt < attempts:
                _LOGGER.warning(
                    "%s on iteration %s/%s", err_msg, attempt, attempts
                )
                continue

            if isinstance(err, urllib.error.URLError) and isinstance(
                err.reason, TimeoutError
            ):
                raise errors.HttpTimeoutError(
                    f"{err_msg} after {timeout}s"
                ) from err

            raise errors.HttpRequestError(err_msg) from err
    raise errors.HttpRequestError(
        f"Failed to send the HTTP request to url={url} with data={data} and "
        f"headers={headers}."
    )
