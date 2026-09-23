#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import unittest

from antlion.controllers.ap_lib.radio_measurement import (
    BssidInformation,
    NeighborReportElement,
    PhyType,
)
from antlion.controllers.ap_lib.wireless_network_management import (
    BssTransitionCandidateList,
    BssTransitionManagementRequest,
)
from honeydew.typing.custom_types import MacAddress

EXPECTED_NEIGHBOR_1 = NeighborReportElement(
    bssid=MacAddress("01:23:45:ab:cd:ef"),
    bssid_information=BssidInformation(),
    operating_class=81,
    channel_number=1,
    phy_type=PhyType.HT,
)
EXPECTED_NEIGHBOR_2 = NeighborReportElement(
    bssid=MacAddress("cd:ef:ab:45:67:89"),
    bssid_information=BssidInformation(),
    operating_class=121,
    channel_number=149,
    phy_type=PhyType.VHT,
)
EXPECTED_NEIGHBORS = [EXPECTED_NEIGHBOR_1, EXPECTED_NEIGHBOR_2]
EXPECTED_CANDIDATE_LIST = BssTransitionCandidateList(EXPECTED_NEIGHBORS)


class WirelessNetworkManagementTest(unittest.TestCase):
    def test_bss_transition_management_request(self):
        request = BssTransitionManagementRequest(
            disassociation_imminent=True,
            abridged=True,
            candidate_list=EXPECTED_NEIGHBORS,
        )
        self.assertTrue(request.disassociation_imminent)
        self.assertTrue(request.abridged)
        self.assertIn(EXPECTED_NEIGHBOR_1, request.candidate_list)
        self.assertIn(EXPECTED_NEIGHBOR_2, request.candidate_list)


if __name__ == "__main__":
    unittest.main()
