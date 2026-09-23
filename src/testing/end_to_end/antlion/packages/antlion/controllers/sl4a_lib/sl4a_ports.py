#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.


class Sl4aPorts(object):
    """A container for the three ports needed for an SL4A connection.

    Attributes:
        client_port: The port on the host associated with the SL4A client
        forwarded_port: The port forwarded to the Android device.
        server_port: The port on the device associated with the SL4A server.
    """

    def __init__(
        self,
        client_port: int = 0,
        forwarded_port: int = 0,
        server_port: int = 0,
    ) -> None:
        self.client_port = client_port
        self.forwarded_port = forwarded_port
        self.server_port = server_port

    def __str__(self) -> str:
        return (
            f"({self.client_port}, {self.forwarded_port}, {self.server_port})"
        )
