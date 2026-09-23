# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains errors raised by WLAN affordances."""

import fidl_fuchsia_wlan_policy as f_wlan_policy
from honeydew import errors


class HoneydewWlanError(errors.HoneydewError):
    """Raised by WLAN affordances."""


class HoneydewWlanRequestRejectedError(HoneydewWlanError):
    """WLAN stack rejected a request.

    Read the `reason` member variable for details on why this request has been
    rejected by the WLAN stack.
    """

    def __init__(
        self, method: str, reason: f_wlan_policy.RequestStatus
    ) -> None:
        """Initialize a HoneydewWlanRequestRejectedError.

        Args:
            name: name of the request that failed
            reason: additional information about the failed request.
        """
        super().__init__(f"{method} rejected with RequestStatus {reason.name}")
        self.reason = reason


class NetworkInterfaceNotFoundError(errors.HoneydewError):
    """Raised when a matching network interface is not found."""
