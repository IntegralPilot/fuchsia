#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import errno
import socket
import time


def wait_for_port(host: str, port: int, timeout_sec: int = 5) -> None:
    """Wait for the host to start accepting connections on the port.

    Some services take some time to start. Call this after launching the service
    to avoid race conditions.

    Args:
        host: IP of the running service.
        port: Port of the running service.
        timeout_sec: Seconds to wait until raising TimeoutError

    Raises:
        TimeoutError: when timeout_sec has expired without a successful
            connection to the service
    """
    last_error: OSError | None = None
    timeout = time.perf_counter() + timeout_sec

    while True:
        try:
            time_left = max(timeout - time.perf_counter(), 0)
            with socket.create_connection((host, port), timeout=time_left):
                return
        except ConnectionRefusedError as e:
            # Occurs when the host is online but not ready to accept connections
            # yet; wait to see if the host becomes ready.
            last_error = e
        except TimeoutError as e:
            last_error = e
        except OSError as e:
            if e.errno == errno.EHOSTUNREACH:
                # No route to host. Occurs when the interface to the host is
                # torn down; wait to see if the interface comes back.
                last_error = e
            else:
                # Unexpected error
                raise e

        if time.perf_counter() >= timeout:
            raise TimeoutError(
                f"Waited over {timeout_sec}s for the service to start "
                f"accepting connections at {host}:{port}"
            ) from last_error
