# Copyright (C) 2024 The Android Open Source Project
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#      http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.
"""SDP proxy module."""

import unittest
from itertools import filterfalse

from mmi2grpc._helpers import assert_description, match_description
from mmi2grpc._proxy import ProfileProxy
from pandora.l2cap_grpc import L2CAP

# As per Bluetooth Assigned Numbers
UUID_TO_SERVICE_NAME = {
    # Service Classes and Profiles
    0x1105: "OBEXObjectPush",
    0x110A: "AudioSource",
    0x110C: "A/V_RemoteControlTarget",
    0x110E: "A/V_RemoteControl",
    0x110F: "A/V_RemoteControlController",
    0x1112: "Headset - Audio Gateway",
    0x1115: "PANU",
    0x1116: "NAP",
    0x111F: "HandsfreeAudioGateway",
    0x112F: "Phonebook Access - PSE",
    0x1132: "Message Access Server",
    0x1203: "GenericAudio",
    # GATT Service
    0x1800: "Generic Access",
    0x1801: "Generic Attribute service",
    0x1855: "TMAS",
    # Custom UUIDs
    0xC26CF572_3369_4CF2_B5CC_D2CD130F5B2C: "Android Auto Compatibility",
}


class SDPProxy(ProfileProxy):
    def __init__(self, channel: str):
        super().__init__()

        self.l2cap = L2CAP(channel)

    @assert_description
    def _mmi_6000(self, **kwargs):
        """
        If necessary take action to accept the SDP channel connection.
        """

        return self.TSC_SDP_mmi_iut_accept_connection(**kwargs)

    @assert_description
    def TSC_SDP_mmi_iut_accept_connection(self, **kwargs):
        """
        If necessary take action to accept the SDP channel connection.
        """

        return "OK"

    @assert_description
    def _mmi_6001(self, **kwargs):
        """
        If necessary take action to respond to the Service Attribute operation
        appropriately.
        """

        return self.TSC_SDP_mmi_iut_accept_service_attribute(**kwargs)

    @assert_description
    def TSC_SDP_mmi_iut_accept_service_attribute(self, **kwargs):
        """
        If necessary take action to respond to the Service Attribute operation
        appropriately.
        """

        return "OK"

    @assert_description
    def _mmi_6002(self, **kwargs):
        """
        If necessary take action to accept the Service Search operation.
        """

        return self.TSC_SDP_mmi_iut_accept_service_search(**kwargs)

    @assert_description
    def TSC_SDP_mmi_iut_accept_service_search(self, **kwargs):
        """
        If necessary take action to accept the Service Search operation.
        """

        self.l2cap.WaitConnection()

        return "OK"

    @assert_description
    def _mmi_6003(self, **kwargs):
        """
        If necessary take action to respond to the Service Search Attribute
        operation appropriately.
        """

        return self.TSC_SDP_mmi_iut_accept_service_search_attribute(**kwargs)

    @assert_description
    def TSC_SDP_mmi_iut_accept_service_search_attribute(self, **kwargs):
        """
        If necessary take action to respond to the Service Search Attribute
        operation appropriately.
        """

        return "OK"

    @match_description
    def TSC_SDP_mmi_verify_browsable_services(self, uuids, **kwargs):
        r"""
        Are all browsable service classes listed below\?

        (?P<uuids>(?:0x[0-9A-F]+, )+0x[0-9A-F]+)
        """

        uuid_list = uuids.split(", ")
        reported_services = list(
            map(
                lambda uuid: UUID_TO_SERVICE_NAME.get(int(uuid, 16), uuid),
                uuid_list,
            )
        )
        test = unittest.TestCase()

        # yapf: disable
        expected_services = [
            "Generic Attribute service",
            "Generic Access",
            "AudioSource",
            "A/V_RemoteControlTarget",
            "A/V_RemoteControl",
            "A/V_RemoteControlController",
            "Headset - Audio Gateway",
            "GenericAudio",
            "HandsfreeAudioGateway",
            "GenericAudio",
            "Message Access Server",
            "NAP",
            "PANU",
            "TMAS",
            "Phonebook Access - PSE",
            "OBEXObjectPush",
            "Android Auto Compatibility",
        ]
        optional_services = [
            "Generic Attribute service",
            "A/V_RemoteControlController",
            "Android Auto Compatibility",
            "TMAS",
        ]
        # Service that can be in any order. This should never be extended
        movable_services = [
            "Message Access Server",
            "TMAS", # TODO: b/341385684 Remove from movable
        ]
        # yapf: enable

        # Remove optional services from expected & movable list
        optional_not_present = (
            lambda service: service in optional_services
            and service not in reported_services
        )
        expected_services = list(
            filterfalse(optional_not_present, expected_services)
        )
        movable_services = list(
            filterfalse(optional_not_present, movable_services)
        )

        # 1st: Check that the movable service are present in whatever order:
        movable_services_names_without_optional = list(
            filter(lambda x: x in movable_services, reported_services)
        )
        test.assertCountEqual(
            movable_services_names_without_optional, movable_services
        )

        # 2nd: Check that all the services except the movable are in the specified order
        reported_services = list(
            filterfalse(lambda key: key in movable_services, reported_services)
        )
        expected_services_without_movable = list(
            filterfalse(lambda key: key in movable_services, expected_services)
        )
        test.assertEqual(reported_services, expected_services_without_movable)

        return "OK"
