from typing import Optional

from mmi2grpc._helpers import assert_description
from mmi2grpc._proxy import ProfileProxy
from pandora.host_grpc import Host
from pandora.host_pb2 import Connection

# from pandora.gavdp_grpc import GAVDP


class GAVDPProxy(ProfileProxy):
    connection: Optional[Connection] = None

    def __init__(self, channel):
        super().__init__()

        self.host = Host(channel)
        # self.gavdp = GAVDP(channel)

    @assert_description
    def test_started(self, **kwargs):
        """ """

        return "OK"

    @assert_description
    def TSC_GAVDP_mmi_sdp_search(self, **kwargs):
        """
        Please wait while PTS queries the IUT SDP record.
        """

        return "OK"

    @assert_description
    def TSC_AVDTP_mmi_iut_initiate_connect(self, pts_addr: bytes, **kwargs):
        """
        Create an AVDTP signaling channel.

        Action: Create an audio or video
        connection with PTS.
        """
        self.connection = self.host.Connect(address=pts_addr).connection

        return "OK"

    @assert_description
    def TSC_AVDTP_mmi_iut_confirm_streaming(self, **kwargs):
        """
        Is the IUT (Implementation Under Test) receiving streaming media from
        PTS?

        Action: Press 'Yes' if the IUT is receiving streaming data from
        the PTS (in some cases the sound may not be clear, this is normal).
        """

        return "Yes"
