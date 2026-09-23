#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import operator
import time
from typing import Any

from antlion import context, utils
from antlion.controllers.access_point import AccessPoint, setup_ap
from antlion.controllers.ap_lib import hostapd_constants
from antlion.controllers.ap_lib.hostapd_security import (
    Security as DeprecatedSecurity,
)
from antlion.test_utils.abstract_devices import wmm_transceiver
from antlion.test_utils.abstract_devices.wlan_device import FuchsiaWlanDevice
from antlion.test_utils.fuchsia import wmm_test_cases
from fuchsia_wlan_base_test.deprecated.wifi import base_test
from mobly import asserts, test_runner
from openwrt_access_point import OpenWrtAP
from openwrt_access_point.lib import capabilities
from openwrt_access_point.lib.access_point_config import (
    AccessPointConfig,
    Band,
    BssChannel,
    BssSettings,
    CapabilitySelection,
    HtMode,
    PhyMode,
    RadioConfig,
    SecurityOpen,
    VhtMode,
)
from openwrt_access_point.lib.access_point_config_mapper import (
    AccessPointConfigMapper as ConfigMapper,
)
from openwrt_access_point.lib.hostapd_options import (
    HostapdOptions,
    WmmAcm,
    WmmParams,
)

DEFAULT_N_CAPABILITIES_20_MHZ = [
    capabilities.N_CAPABILITY_LDPC,
    capabilities.N_CAPABILITY_SHORT_GI_20,
    capabilities.N_CAPABILITY_TX_STBC,
    capabilities.N_CAPABILITY_RX_STBC1,
    capabilities.N_CAPABILITY_HT20,
]

DEFAULT_AP_PARAMS = {
    "profile_name": "whirlwind",
    "channel": hostapd_constants.AP_DEFAULT_CHANNEL_2G,
    "n_capabilities": DEFAULT_N_CAPABILITIES_20_MHZ,
    "ac_capabilities": None,
}

DEFAULT_BW_PERCENTAGE = 1
DEFAULT_STREAM_TIMEOUT = 60
DEFAULT_STREAM_TIME = 10

OPERATORS = {
    ">": operator.gt,
    ">=": operator.ge,
    "<": operator.lt,
    "<=": operator.le,
    "==": operator.eq,
}

GRAPH_COLOR_LEN = 10
GRAPH_DEFAULT_LINE_WIDTH = 2
GRAPH_DEFAULT_CIRCLE_SIZE = 10


def eval_operator(
    operator_string: str,
    actual_value: float,
    expected_value: float,
    max_bw: float,
    rel_tolerance: float = 0,
    abs_tolerance: float = 0,
    max_bw_rel_tolerance: float = 0,
) -> bool:
    """
    Determines if an inequality evaluates to True, given relative and absolute
    tolerance.

    Args:
        operator_string: string, the operator to use for the comparison
        actual_value: the value to compare to some expected value
        expected_value: the value the actual value is compared to
        rel_tolerance: decimal representing the percent tolerance, relative to
            the expected value. E.g. (101 <= 100) w/ rel_tol=0.01 is True
        abs_tolerance: the lowest actual (not percent) tolerance for error.
            E.g. (101 == 100) w/ rel_tol=0.005 is False, but
            (101 == 100) w/ rel_tol=0.005 and abs_tol=1 is True
        max_bw_rel_tolerance: decimal representing the percent tolerance,
            relative to the maximimum allowed bandwidth.
            E.g. (101 <= max bw of 100) w/ max_bw_rel_tol=0.01 is True


    Returns:
        True, if inequality evaluates to True within tolerances
        False, otherwise
    """
    op = OPERATORS[operator_string]
    if op(actual_value, expected_value):
        return True

    error = abs(actual_value - expected_value)
    accepted_error = max(
        expected_value * rel_tolerance,
        abs_tolerance,
        max_bw * max_bw_rel_tolerance,
    )
    return error <= accepted_error


class WlanWmmTest(base_test.WifiBaseTest):
    """Tests WMM QoS Functionality (Station only)

    Testbed Requirements:
    * One ACTS compatible wlan_device (staut)
    * One Whirlwind Access Point
    * For some tests, One additional ACTS compatible device (secondary_sta)

    For accurate results, must be performed in an RF isolated environment.
    """

    def setup_class(self) -> None:
        super().setup_class()
        self.log = logging.getLogger()

        try:
            self.wmm_test_params = self.user_params["wmm_test_params"]
            self._wmm_transceiver_configs = self.wmm_test_params[
                "wmm_transceivers"
            ]
        except KeyError:
            raise AttributeError(
                "Must provide at least 2 WmmTransceivers in "
                '"wmm_test_params" field of ACTS config.'
            )

        if len(self._wmm_transceiver_configs) < 2:
            raise AttributeError("At least 2 WmmTransceivers must be provided.")

        self.wlan_devices = [
            FuchsiaWlanDevice(device) for device in self.fuchsia_devices
        ]

        # Create STAUT transceiver
        if "staut" not in self._wmm_transceiver_configs:
            raise AttributeError(
                'Must provide a WmmTransceiver labeled "staut" with a '
                "wlan_device."
            )
        self.staut = wmm_transceiver.create(
            self._wmm_transceiver_configs["staut"],
            identifier="staut",
            wlan_devices=self.wlan_devices,
        )

        # Required to for automated power cycling
        self.dut = self.staut.wlan_device

        # Create AP transceiver
        if "access_point" not in self._wmm_transceiver_configs:
            raise AttributeError(
                'Must provide a WmmTransceiver labeled "access_point" with a '
                "access_point."
            )
        self.access_point_transceiver = wmm_transceiver.create(
            self._wmm_transceiver_configs["access_point"],
            identifier="access_point",
            access_points=self.openwrt_aps
            if self.openwrt_aps
            else self.access_points,
        )

        self.wmm_transceivers = [self.staut, self.access_point_transceiver]

        # Create secondary station transceiver, if present
        if "secondary_sta" in self._wmm_transceiver_configs:
            self.secondary_sta = wmm_transceiver.create(
                self._wmm_transceiver_configs["secondary_sta"],
                identifier="secondary_sta",
                wlan_devices=self.wlan_devices,
            )
            self.wmm_transceivers.append(self.secondary_sta)
        else:
            self.secondary_sta = None

        self.wmm_transceiver_map = {
            tc.identifier: tc for tc in self.wmm_transceivers
        }

    def setup_test(self) -> None:
        super().setup_test()
        for tc in self.wmm_transceivers:
            if tc.wlan_device:
                tc.wlan_device.disconnect()
            if isinstance(tc.access_point, AccessPoint):
                tc.access_point.stop_all_aps()

    def teardown_test(self) -> None:
        for tc in self.wmm_transceivers:
            tc.cleanup_asynchronous_streams()
            if tc.wlan_device:
                tc.wlan_device.disconnect()
            self.download_logs()
            if isinstance(tc.access_point, AccessPoint):
                tc.access_point.stop_all_aps()
        super().teardown_test()

    def teardown_class(self) -> None:
        for tc in self.wmm_transceivers:
            tc.destroy_resources()
        super().teardown_class()

    def start_ap_with_wmm_params(
        self,
        ap_parameters: dict[str, Any],
        wmm_parameters: HostapdOptions,
    ) -> str:
        """Sets up WMM network on AP.

        Args:
            ap_parameters: a dictionary of kwargs to set up on ap
            wmm_parameters: a dictionary of wmm_params to set up on ap

        Returns:
            String, subnet of the network setup (e.g. '192.168.1.0/24')
        """
        # Defaults for required parameters
        ap_parameters["force_wmm"] = True
        if "ssid" not in ap_parameters:
            ap_parameters["ssid"] = utils.rand_ascii_str(
                hostapd_constants.AP_SSID_LENGTH_2G
            )

        if "profile_name" not in ap_parameters:
            ap_parameters["profile_name"] = "whirlwind"

        if "channel" not in ap_parameters:
            ap_parameters["channel"] = 6

        if "n_capabilities" not in ap_parameters:
            ap_parameters["n_capabilities"] = DEFAULT_N_CAPABILITIES_20_MHZ
        if "additional_ap_parameters" in ap_parameters:
            ap_parameters["additional_ap_parameters"].update(wmm_parameters)
        else:
            ap_parameters["additional_ap_parameters"] = wmm_parameters

        ap = self.access_point_transceiver.access_point
        if isinstance(ap, OpenWrtAP):
            channel_num = ap_parameters["channel"]
            phy_mode: PhyMode
            if channel_num < hostapd_constants.LOWEST_5G_CHANNEL:
                band = Band.BAND_2G
                phy_mode = HtMode(bw=20)
            else:
                band = Band.BAND_5G
                phy_mode = VhtMode(bw=80)

            n_caps = ap_parameters.get("n_capabilities", [])
            ac_caps = ap_parameters.get("ac_capabilities", [])

            security = ap_parameters.get("security", SecurityOpen())
            password = ap_parameters.get("password", None)

            config = AccessPointConfig(
                radios=[
                    RadioConfig(
                        channel=BssChannel(
                            band=band, number=channel_num, phy_mode=phy_mode
                        ),
                        bss_settings=[
                            BssSettings(
                                ssid=ap_parameters["ssid"],
                                security=security,
                                password=password,
                            )
                        ],
                        n_capabilities=CapabilitySelection.CUSTOM(n_caps)
                        if n_caps
                        else CapabilitySelection.DEFAULT(),
                        ac_capabilities=CapabilitySelection.CUSTOM(ac_caps)
                        if ac_caps
                        else CapabilitySelection.DEFAULT(),
                        custom_hostapd_options=wmm_parameters,
                    )
                ]
            )
            ap.configure_wifi(config)
            self.log.info(f"OpenWRT Network ({ap_parameters['ssid']}) is up.")
            return ap.default_subnet
        elif isinstance(ap, AccessPoint):
            security = ap_parameters.get("security")
            if security:
                ap_parameters["security"] = DeprecatedSecurity(
                    security_mode=ConfigMapper.to_hostapd_security(security),
                    password=ap_parameters.get("password", None),
                )

            # Map capabilities
            if ap_parameters.get("n_capabilities") is not None:
                ap_parameters["n_capabilities"] = [
                    ConfigMapper.to_hostapd_n_cap(cap)
                    for cap in ap_parameters["n_capabilities"]
                ]
            if ap_parameters.get("ac_capabilities") is not None:
                ap_parameters["ac_capabilities"] = [
                    ConfigMapper.to_hostapd_ac_cap(cap)
                    for cap in ap_parameters["ac_capabilities"]
                ]

            # Start AP with kwargs
            self.log.info(f"Setting up WMM network: {ap_parameters['ssid']}")
            setup_ap(ap, **ap_parameters)
            self.log.info(f"Network ({ap_parameters['ssid']}) is up.")

            # Return subnet
            if ap_parameters["channel"] < hostapd_constants.LOWEST_5G_CHANNEL:
                return ap._AP_2G_SUBNET_STR
            else:
                return ap._AP_5G_SUBNET_STR
        else:
            raise TypeError("Unsupported access point type.")

    def associate_transceiver(
        self, wmm_transceiver: Any, ap_params: dict[str, Any]
    ) -> None:
        """Associates a WmmTransceiver that has a wlan_device.

        Args:
            wmm_transceiver: transceiver to associate
            ap_params: dict, contains ssid and password, if any, for network
        """
        if not wmm_transceiver.wlan_device:
            raise AttributeError(
                "Cannot associate a WmmTransceiver that does not have a WLAN device."
            )
        ssid = ap_params["ssid"]
        security = ap_params.get("security", SecurityOpen())
        password = ap_params.get("password", None)
        target_security = ConfigMapper.to_hostapd_security(security)
        associated = wmm_transceiver.wlan_device.associate(
            ssid,
            target_security,
            target_pwd=password,
        )
        if not associated:
            raise ConnectionError(
                f"Failed to associate WmmTransceiver {wmm_transceiver.identifier}."
            )
        self.log.info(
            f"WmmTransceiver {wmm_transceiver.identifier} associated."
        )

    def validate_streams_in_phase(
        self, phase_id: str, phases: dict[str, Any], max_bw: float
    ) -> bool:
        """Validates any stream in a phase that has validation criteria.

        Args:
            phase_id: identifier of the phase to check
            phases: dictionary containing phases for retrieving stream
                transmitters, expected bandwidths, etc.
            max_bw: the max link bandwidth, measured in the test

        Returns:
            True, if ALL validation criteria for ALL streams in phase pass
            False, otherwise
        """
        pass_val = True
        for stream_id, stream in phases[phase_id].items():
            if "validation" in stream:
                transmitter = stream["transmitter"]
                uuid = stream["uuid"]
                actual_bw = transmitter.get_results(uuid).avg_rate
                if not actual_bw:
                    raise ConnectionError(
                        "(Phase: %s, Stream: %s) - Stream results show "
                        "bandwidth: None" % (phase_id, stream_id)
                    )
                for check in stream["validation"]:
                    operator_str = check["operator"]
                    rel_tolerance = check.get("rel_tolerance", 0)
                    abs_tolerance = check.get("abs_tolerance", 0)
                    max_bw_rel_tolerance = check.get("max_bw_rel_tolerance", 0)
                    expected_bw_percentage = check.get(
                        "bandwidth_percentage", DEFAULT_BW_PERCENTAGE
                    )
                    # Explicit Bandwidth Validation
                    if "bandwidth" in check:
                        comp_bw = check["bandwidth"]
                        log_msg = (
                            "Expected Bandwidth: %s (explicit validation "
                            "bandwidth [%s] x expected bandwidth "
                            "percentage [%s])"
                            % (
                                expected_bw_percentage * comp_bw,
                                comp_bw,
                                expected_bw_percentage,
                            )
                        )

                    # Stream Comparison Validation
                    elif "phase" in check and "stream" in check:
                        comp_phase_id = check["phase"]
                        comp_stream_id = check["stream"]
                        comp_stream = phases[comp_phase_id][comp_stream_id]
                        comp_transmitter = comp_stream["transmitter"]
                        comp_uuid = comp_stream["uuid"]
                        comp_bw = comp_transmitter.get_results(
                            comp_uuid
                        ).avg_rate
                        log_msg = (
                            "Expected Bandwidth: %s (bandwidth for phase: %s, "
                            "stream: %s [%s] x expected bandwidth percentage "
                            "[%s])"
                            % (
                                expected_bw_percentage * comp_bw,
                                comp_phase_id,
                                comp_stream_id,
                                comp_bw,
                                expected_bw_percentage,
                            )
                        )

                    # Expected Bandwidth Validation
                    else:
                        if "bandwidth" in stream:
                            comp_bw = stream["bandwidth"]
                            log_msg = (
                                "Expected Bandwidth: %s (expected stream "
                                "bandwidth [%s] x expected bandwidth "
                                "percentage [%s])"
                                % (
                                    expected_bw_percentage * comp_bw,
                                    comp_bw,
                                    expected_bw_percentage,
                                )
                            )
                        else:
                            max_bw_percentage = stream.get(
                                "max_bandwidth_percentage",
                                DEFAULT_BW_PERCENTAGE,
                            )
                            comp_bw = max_bw * max_bw_percentage
                            log_msg = (
                                "Expected Bandwidth: %s (max bandwidth [%s] x "
                                "stream bandwidth percentage [%s] x expected "
                                "bandwidth percentage [%s])"
                                % (
                                    expected_bw_percentage * comp_bw,
                                    max_bw,
                                    max_bw_percentage,
                                    expected_bw_percentage,
                                )
                            )

                    self.log.info(
                        "Validation criteria - Stream: %s, "
                        "Actual Bandwidth: %s, Operator: %s, %s, "
                        "Relative Tolerance: %s, Absolute Tolerance: %s, Max "
                        "Bandwidth Relative Tolerance: %s"
                        % (
                            stream_id,
                            actual_bw,
                            operator_str,
                            log_msg,
                            rel_tolerance,
                            abs_tolerance,
                            max_bw_rel_tolerance,
                        )
                    )

                    if eval_operator(
                        operator_str,
                        actual_bw,
                        comp_bw * expected_bw_percentage,
                        max_bw,
                        rel_tolerance=rel_tolerance,
                        abs_tolerance=abs_tolerance,
                        max_bw_rel_tolerance=max_bw_rel_tolerance,
                    ):
                        self.log.info(
                            "(Phase: %s, Stream: %s) - PASSES validation check!"
                            % (phase_id, stream_id)
                        )
                    else:
                        self.log.info(
                            "(Phase: %s, Stream: %s) - Stream FAILS validation "
                            "check." % (phase_id, stream_id)
                        )
                        pass_val = False
        if pass_val:
            self.log.info(
                f"(Phase {phase_id}) - All streams' validation criteria were met."
            )
            return True
        else:
            self.log.error(
                "(Phase %s) - At least one stream validation criterion was not "
                "met." % phase_id
            )
            return False

    def graph_test(self, phases: dict[str, Any], max_bw: float) -> None:
        """Outputs a bokeh html graph of the streams. Saves to ACTS log
        directory.

        Args:
            phases: dictionary containing phases for retrieving stream
                transmitters, expected bandwidths, etc.
            max_bw: the max link bandwidth, measured in the test

        """

        try:
            from bokeh.models import Label, Span
            from bokeh.palettes import Category10
            from bokeh.plotting import (
                ColumnDataSource,
                figure,
                output_file,
                save,
            )
        except ImportError:
            self.log.warn(
                "bokeh is not installed: skipping creation of graphs. "
                "Note CSV files are still available. If graphs are "
                'desired, install antlion with the "bokeh" feature.'
            )
            return

        output_path = context.get_current_context().get_base_output_path()
        output_file_name = "%s/WlanWmmTest/%s.html" % (
            output_path,
            self.current_test_info.name,
        )
        output_file(output_file_name)

        start_time = 0
        graph_lines = []

        # Used for scaling
        highest_stream_bw = 0
        lowest_stream_bw = 100000

        for phase_id, phase in phases.items():
            longest_stream_time = 0
            for stream_id, stream in phase.items():
                transmitter = stream["transmitter"]
                uuid = stream["uuid"]

                if "bandwidth" in stream:
                    stream_bw = f"{stream['bandwidth']:.3f}"
                    stream_bw_formula_str = f"{stream_bw}Mb/s"
                elif "max_bandwidth_percentage" in stream:
                    max_bw_percentage = stream["max_bandwidth_percentage"]
                    stream_bw = f"{max_bw * max_bw_percentage:.3f}"
                    stream_bw_formula_str = "%sMb/s (%s%% of max bandwidth)" % (
                        stream_bw,
                        str(max_bw_percentage * 100),
                    )
                else:
                    raise AttributeError(
                        "Stream %s must have either a bandwidth or "
                        "max_bandwidth_percentage parameter." % stream_id
                    )

                stream_time = stream.get("time", DEFAULT_STREAM_TIME)
                longest_stream_time = max(longest_stream_time, stream_time)

                avg_rate = transmitter.get_results(uuid).avg_rate

                instantaneous_rates = transmitter.get_results(
                    uuid
                ).instantaneous_rates
                highest_stream_bw = max(
                    highest_stream_bw, max(instantaneous_rates)
                )
                lowest_stream_bw = min(
                    lowest_stream_bw, min(instantaneous_rates)
                )

                stream_data = ColumnDataSource(
                    dict(
                        time=[
                            x
                            for x in range(start_time, start_time + stream_time)
                        ],
                        instantaneous_bws=instantaneous_rates,
                        avg_bw=[avg_rate for _ in range(stream_time)],
                        stream_id=[stream_id for _ in range(stream_time)],
                        attempted_bw=[
                            stream_bw_formula_str for _ in range(stream_time)
                        ],
                    )
                )
                line = {
                    "x_axis": "time",
                    "y_axis": "instantaneous_bws",
                    "source": stream_data,
                    "line_width": GRAPH_DEFAULT_LINE_WIDTH,
                    "legend_label": f"{phase_id}:{stream_id}",
                }
                graph_lines.append(line)

            start_time = start_time + longest_stream_time
        TOOLTIPS = [
            ("Time", "@time"),
            ("Attempted Bandwidth", "@attempted_bw"),
            ("Instantaneous Bandwidth", "@instantaneous_bws"),
            ("Stream Average Bandwidth", "@avg_bw"),
            ("Stream", "@stream_id"),
        ]

        # Create and scale graph appropriately
        time_vs_bandwidth_graph = figure(
            title=f"Bandwidth for {self.current_test_info.name}",
            x_axis_label="Time",
            y_axis_label="Bandwidth",
            tooltips=TOOLTIPS,
            y_range=(
                lowest_stream_bw
                - (0.5 * (highest_stream_bw - lowest_stream_bw)),
                1.05 * max_bw,
            ),
        )
        time_vs_bandwidth_graph.sizing_mode = "stretch_both"
        time_vs_bandwidth_graph.title.align = "center"
        colors = Category10[GRAPH_COLOR_LEN]
        color_ind = 0

        # Draw max bandwidth line
        max_bw_span = Span(
            location=max_bw,
            dimension="width",
            line_color="black",
            line_dash="dashed",
            line_width=GRAPH_DEFAULT_LINE_WIDTH,
        )
        max_bw_label = Label(
            x=(0.5 * start_time),
            y=max_bw,
            text=f"Max Bandwidth: {max_bw}Mb/s",
            text_align="center",
        )
        time_vs_bandwidth_graph.add_layout(max_bw_span)
        time_vs_bandwidth_graph.add_layout(max_bw_label)

        # Draw stream lines
        for line in graph_lines:
            time_vs_bandwidth_graph.line(
                line["x_axis"],
                line["y_axis"],
                source=line["source"],
                line_width=line["line_width"],
                legend_label=line["legend_label"],
                color=colors[color_ind],
            )
            time_vs_bandwidth_graph.circle(
                line["x_axis"],
                line["y_axis"],
                source=line["source"],
                size=GRAPH_DEFAULT_CIRCLE_SIZE,
                legend_label=line["legend_label"],
                color=colors[color_ind],
            )
            color_ind = (color_ind + 1) % GRAPH_COLOR_LEN
        time_vs_bandwidth_graph.legend.location = "top_left"
        time_vs_bandwidth_graph.legend.click_policy = "hide"
        graph_file = save([time_vs_bandwidth_graph])
        self.log.info(f"Saved graph to {graph_file}")

    def run_wmm_test(
        self,
        phases: dict[str, Any],
        ap_parameters: dict[str, Any] = DEFAULT_AP_PARAMS,
        wmm_parameters: HostapdOptions = WmmParams.DEFAULT_PHYS_11A_11G_11N_11AC_DEFAULT_PARAMS,
        stream_timeout: int = DEFAULT_STREAM_TIMEOUT,
    ) -> None:
        """Runs a WMM test case.

        Args:
            phases: dictionary of phases of streams to run in parallel,
                including any validation critera (see example below).
            ap_parameters: dictionary of custom kwargs to setup on AP (see
                start_ap_with_wmm_parameters)
            wmm_parameters: dictionary of WMM AC parameters
            stream_timeout: int, time in seconds to wait before force joining
                parallel streams

        Asserts:
            PASS, if all validation criteria for all phases are met
            FAIL, otherwise
        """
        # Setup AP
        subnet_str = self.start_ap_with_wmm_params(
            ap_parameters, wmm_parameters
        )
        # Determine transmitters and receivers used in test case
        transmitters = set()
        receivers = set()
        for phase in phases.values():
            for stream in phase.values():
                transmitter = self.wmm_transceiver_map[
                    stream["transmitter_str"]
                ]
                transmitters.add(transmitter)
                stream["transmitter"] = transmitter
                receiver = self.wmm_transceiver_map[stream["receiver_str"]]
                receivers.add(receiver)
                stream["receiver"] = receiver
        transceivers = transmitters.union(receivers)

        # Associate all transceivers with wlan_devices
        for tc in transceivers:
            if tc.wlan_device:
                self.associate_transceiver(tc, ap_parameters)

        # Determine link max bandwidth
        self.log.info("Determining link maximum bandwidth.")
        uuid = self.staut.run_synchronous_traffic_stream(
            {"receiver": self.access_point_transceiver}, subnet_str
        )
        max_bw = self.staut.get_results(uuid).avg_send_rate
        self.log.info(f"Link maximum bandwidth: {max_bw} Mb/s")

        # Run parallel phases
        pass_test = True
        for phase_id, phase in phases.items():
            self.log.info(f"Setting up phase: {phase_id}")

            for stream_id, stream in phase.items():
                transmitter = stream["transmitter"]
                receiver = stream["receiver"]
                access_category = stream.get("access_category", None)
                stream_time = stream.get("time", DEFAULT_STREAM_TIME)

                # Determine stream type
                if "bandwidth" in stream:
                    bw = stream["bandwidth"]
                elif "max_bandwidth_percentage" in stream:
                    max_bw_percentage = stream["max_bandwidth_percentage"]
                    bw = max_bw * max_bw_percentage
                else:
                    raise AttributeError(
                        "Stream %s must have either a bandwidth or "
                        "max_bandwidth_percentage parameter." % stream_id
                    )

                stream_params = {
                    "receiver": receiver,
                    "access_category": access_category,
                    "bandwidth": bw,
                    "time": stream_time,
                }

                uuid = transmitter.prepare_asynchronous_stream(
                    stream_params, subnet_str
                )
                stream["uuid"] = uuid

            # Start all streams in phase
            start_time = time.time() + 5
            for transmitter in transmitters:
                transmitter.start_asynchronous_streams(start_time=start_time)

            # Wait for streams to join
            for transmitter in transmitters:
                end_time = time.time() + stream_timeout
                while transmitter.has_active_streams:
                    if time.time() > end_time:
                        raise ConnectionError(
                            "Transmitter's (%s) active streams are not finishing."
                            % transmitter.identifier
                        )
                    time.sleep(1)

            # Cleanup all streams
            for transmitter in transmitters:
                transmitter.cleanup_asynchronous_streams()

            # Validate streams
            pass_test = pass_test and self.validate_streams_in_phase(
                phase_id, phases, max_bw
            )

        self.graph_test(phases, max_bw)
        if pass_test:
            asserts.explicit_pass(
                "Validation criteria met for all streams in all phases."
            )
        else:
            asserts.fail(
                "At least one stream failed to meet validation criteria."
            )

    # Test Cases

    # Internal Traffic Differentiation

    def test_internal_traffic_diff_VO_VI(self) -> None:
        self.run_wmm_test(wmm_test_cases.test_internal_traffic_diff_VO_VI)

    def test_internal_traffic_diff_VO_BE(self) -> None:
        self.run_wmm_test(wmm_test_cases.test_internal_traffic_diff_VO_BE)

    def test_internal_traffic_diff_VO_BK(self) -> None:
        self.run_wmm_test(wmm_test_cases.test_internal_traffic_diff_VO_BK)

    def test_internal_traffic_diff_VI_BE(self) -> None:
        self.run_wmm_test(wmm_test_cases.test_internal_traffic_diff_VI_BE)

    def test_internal_traffic_diff_VI_BK(self) -> None:
        self.run_wmm_test(wmm_test_cases.test_internal_traffic_diff_VI_BK)

    def test_internal_traffic_diff_BE_BK(self) -> None:
        self.run_wmm_test(wmm_test_cases.test_internal_traffic_diff_BE_BK)

    # External Traffic Differentiation

    """Single station, STAUT transmits high priority"""

    def test_external_traffic_diff_staut_VO_ap_VI(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_VO_ap_VI
        )

    def test_external_traffic_diff_staut_VO_ap_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_VO_ap_BE
        )

    def test_external_traffic_diff_staut_VO_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_VO_ap_BK
        )

    def test_external_traffic_diff_staut_VI_ap_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_VI_ap_BE
        )

    def test_external_traffic_diff_staut_VI_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_VI_ap_BK
        )

    def test_external_traffic_diff_staut_BE_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_BE_ap_BK
        )

    """Single station, STAUT transmits low priority"""

    def test_external_traffic_diff_staut_VI_ap_VO(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_VI_ap_VO
        )

    def test_external_traffic_diff_staut_BE_ap_VO(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_BE_ap_VO
        )

    def test_external_traffic_diff_staut_BK_ap_VO(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_BK_ap_VO
        )

    def test_external_traffic_diff_staut_BE_ap_VI(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_BE_ap_VI
        )

    def test_external_traffic_diff_staut_BK_ap_VI(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_BK_ap_VI
        )

    def test_external_traffic_diff_staut_BK_ap_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_external_traffic_diff_staut_BK_ap_BE
        )

    # # Dual Internal/External Traffic Differentiation (Single station)

    def test_dual_traffic_diff_staut_VO_VI_ap_VI(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_dual_traffic_diff_staut_VO_VI_ap_VI
        )

    def test_dual_traffic_diff_staut_VO_BE_ap_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_dual_traffic_diff_staut_VO_BE_ap_BE
        )

    def test_dual_traffic_diff_staut_VO_BK_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_dual_traffic_diff_staut_VO_BK_ap_BK
        )

    def test_dual_traffic_diff_staut_VI_BE_ap_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_dual_traffic_diff_staut_VI_BE_ap_BE
        )

    def test_dual_traffic_diff_staut_VI_BK_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_dual_traffic_diff_staut_VI_BK_ap_BK
        )

    def test_dual_traffic_diff_staut_BE_BK_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_dual_traffic_diff_staut_BE_BK_ap_BK
        )

    # ACM Bit Conformance Tests (Single station, as WFA test below uses two)

    def test_acm_bit_on_VI(self) -> None:
        wmm_params_VI_ACM = (
            WmmParams.DEFAULT_PHYS_11A_11G_11N_11AC_DEFAULT_PARAMS | WmmAcm.VI
        )
        self.run_wmm_test(
            wmm_test_cases.test_acm_bit_on_VI, wmm_parameters=wmm_params_VI_ACM
        )

    # AC Parameter Modificiation Tests (Single station, as WFA test below uses two)

    def test_ac_param_degrade_VO(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_ac_param_degrade_VO,
            wmm_parameters=WmmParams.DEGRADED_VO,
        )

    def test_ac_param_degrade_VI(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_ac_param_degrade_VI,
            wmm_parameters=WmmParams.DEGRADED_VI,
        )

    def test_ac_param_improve_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_ac_param_improve_BE,
            wmm_parameters=WmmParams.IMPROVE_BE,
        )

    def test_ac_param_improve_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_ac_param_improve_BK,
            wmm_parameters=WmmParams.IMPROVE_BK,
        )

    # WFA Test Plan Tests

    """Traffic Differentiation in Single BSS (Single Station)"""

    def test_wfa_traffic_diff_single_station_staut_BE_ap_VI_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_single_station_staut_BE_ap_VI_BE
        )

    def test_wfa_traffic_diff_single_station_staut_VI_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_single_station_staut_VI_BE
        )

    def test_wfa_traffic_diff_single_station_staut_VI_BE_ap_BE(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_single_station_staut_VI_BE_ap_BE
        )

    def test_wfa_traffic_diff_single_station_staut_BE_BK_ap_BK(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_single_station_staut_BE_BK_ap_BK
        )

    def test_wfa_traffic_diff_single_station_staut_VO_VI_ap_VI(self) -> None:
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_single_station_staut_VO_VI_ap_VI
        )

    """Traffic Differentiation in Single BSS (Two Stations)"""

    def test_wfa_traffic_diff_two_stations_staut_BE_secondary_VI_BE(
        self,
    ) -> None:
        asserts.skip_if(not self.secondary_sta, "No secondary station.")
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_two_stations_staut_BE_secondary_VI_BE
        )

    def test_wfa_traffic_diff_two_stations_staut_VI_secondary_BE(self) -> None:
        asserts.skip_if(not self.secondary_sta, "No secondary station.")
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_two_stations_staut_VI_secondary_BE
        )

    def test_wfa_traffic_diff_two_stations_staut_BK_secondary_BE_BK(
        self,
    ) -> None:
        asserts.skip_if(not self.secondary_sta, "No secondary station.")
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_two_stations_staut_BK_secondary_BE_BK
        )

    def test_wfa_traffic_diff_two_stations_staut_VI_secondary_VO_VI(
        self,
    ) -> None:
        asserts.skip_if(not self.secondary_sta, "No secondary station.")
        self.run_wmm_test(
            wmm_test_cases.test_wfa_traffic_diff_two_stations_staut_VI_secondary_VO_VI
        )

    """Test ACM Bit Conformance (Two Stations)"""

    def test_wfa_acm_bit_on_VI(self) -> None:
        asserts.skip_if(not self.secondary_sta, "No secondary station.")
        wmm_params_VI_ACM = (
            WmmParams.DEFAULT_PHYS_11A_11G_11N_11AC_DEFAULT_PARAMS | WmmAcm.VI
        )
        self.run_wmm_test(
            wmm_test_cases.test_wfa_acm_bit_on_VI,
            wmm_parameters=wmm_params_VI_ACM,
        )

    """Test the AC Parameter Modification"""

    def test_wfa_ac_param_degrade_VI(self) -> None:
        asserts.skip_if(not self.secondary_sta, "No secondary station.")
        self.run_wmm_test(
            wmm_test_cases.test_wfa_ac_param_degrade_VI,
            wmm_parameters=WmmParams.DEGRADED_VI,
        )


if __name__ == "__main__":
    test_runner.main()
