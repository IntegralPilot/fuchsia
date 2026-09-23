# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
import unittest

from fuchsia_controller_py import FcTransportStatus, ZxStatus


class Errors(unittest.TestCase):
    """Fuchsia Controller ZxStatus error tests"""

    def test_str_format(self) -> None:
        for test_case in [
            "ZX_ERR_INTERNAL",
            "ZX_ERR_NOT_SUPPORTED",
            "ZX_ERR_NO_RESOURCES",
            "ZX_ERR_NO_MEMORY",
            "ZX_ERR_INVALID_ARGS",
            "ZX_ERR_BAD_HANDLE",
            "ZX_ERR_WRONG_TYPE",
            "ZX_ERR_BAD_SYSCALL",
            "ZX_ERR_OUT_OF_RANGE",
            "ZX_ERR_BUFFER_TOO_SMALL",
            "ZX_ERR_BAD_STATE",
            "ZX_ERR_TIMED_OUT",
            "ZX_ERR_SHOULD_WAIT",
            "ZX_ERR_CANCELED",
            "ZX_ERR_PEER_CLOSED",
            "ZX_ERR_NOT_FOUND",
            "ZX_ERR_ALREADY_EXISTS",
            "ZX_ERR_ALREADY_BOUND",
            "ZX_ERR_UNAVAILABLE",
            "ZX_ERR_ACCESS_DENIED",
            "ZX_ERR_IO",
            "ZX_ERR_IO_REFUSED",
            "ZX_ERR_IO_DATA_INTEGRITY",
            "ZX_ERR_IO_DATA_LOSS",
            "ZX_ERR_IO_NOT_PRESENT",
            "ZX_ERR_IO_OVERRUN",
            "ZX_ERR_IO_MISSED_DEADLINE",
            "ZX_ERR_IO_INVALID",
            "ZX_ERR_BAD_PATH",
            "ZX_ERR_NOT_DIR",
            "ZX_ERR_NOT_FILE",
            "ZX_ERR_FILE_BIG",
            "ZX_ERR_NO_SPACE",
            "ZX_ERR_NOT_EMPTY",
            "ZX_ERR_STOP",
            "ZX_ERR_NEXT",
            "ZX_ERR_ASYNC",
            "ZX_ERR_PROTOCOL_NOT_SUPPORTED",
            "ZX_ERR_ADDRESS_UNREACHABLE",
            "ZX_ERR_ADDRESS_IN_USE",
            "ZX_ERR_NOT_CONNECTED",
            "ZX_ERR_CONNECTION_REFUSED",
            "ZX_ERR_CONNECTION_RESET",
            "ZX_ERR_CONNECTION_ABORTED",
        ]:
            err = ZxStatus(ZxStatus.__dict__[test_case])
            self.assertEqual(repr(err), test_case)

    def test_raw_value_matches(self) -> None:
        err = ZxStatus(ZxStatus.ZX_ERR_NEXT)
        self.assertEqual(err.raw(), ZxStatus.ZX_ERR_NEXT)

    def test_fc_status(self) -> None:
        err = FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL)
        self.assertEqual(err.code(), FcTransportStatus.FC_ERR_INTERNAL)
        self.assertEqual(err.desc(), None)

    def test_fc_status_non_string_description(self) -> None:
        err = FcTransportStatus(FcTransportStatus.FC_ERR_INTERNAL, 1)
        with self.assertRaises(RuntimeError):
            _ = err.desc()

    def test_fc_status_unknown(self) -> None:
        """Tests that the unknown string shows the ZX status if possible."""
        # This test might be a bit fragile if we expand error types for
        # FcTransportStatus, however it's good to ensure users who may have raised the
        # wrong error type can have it printed in a way that is debuggable.
        err = FcTransportStatus(ZxStatus.ZX_ERR_PEER_CLOSED)
        s = str(err)
        # Full string is
        # "(UNKNOWN: -24. As ZxStatus this is: ZX_ERR_PEER_CLOSED)"
        # But we want to prevent making this check overly fragile.
        self.assertTrue("UNKNOWN" in s)
        self.assertTrue("ZX_ERR_PEER_CLOSED" in s)
