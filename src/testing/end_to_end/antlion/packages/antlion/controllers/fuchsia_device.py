#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

from __future__ import annotations

import logging
import os
import re
import socket
import textwrap
import time
from ipaddress import ip_address
from typing import Any, Tuple

import fuchsia_async_extension
import honeydew
from antlion import context, utils
from antlion.capabilities.ssh import DEFAULT_SSH_PORT, SSHConfig
from antlion.controllers import pdu
from antlion.controllers.fuchsia_lib.lib_controllers.wlan_controller import (
    WlanController,
)
from antlion.controllers.fuchsia_lib.lib_controllers.wlan_policy_controller import (
    WlanPolicyController,
)
from antlion.controllers.fuchsia_lib.ssh import (
    DEFAULT_SSH_PRIVATE_KEY,
    DEFAULT_SSH_USER,
    FuchsiaSSHProvider,
)
from antlion.decorators import cached_property
from antlion.types import ControllerConfig, Json
from antlion.utils import (
    get_fuchsia_mdns_ipv6_address,
    get_interface_ip_addresses,
)
from antlion.validation import MapValidator
from honeydew.affordances.connectivity.wlan.utils.types import CountryCode
from honeydew.auxiliary_devices.power_switch.power_switch_using_dmc import (
    PowerSwitchDmcError,
    PowerSwitchUsingDmc,
)
from honeydew.transports.ffx.config import FfxConfig
from honeydew.transports.ffx.ffx import FFX
from honeydew.typing.custom_types import DeviceInfo, FidlEndpoint, IpPort
from mobly import logger, signals

MOBLY_CONTROLLER_CONFIG_NAME: str = "FuchsiaDevice"
ACTS_CONTROLLER_REFERENCE_NAME = "fuchsia_devices"

FUCHSIA_RECONNECT_AFTER_REBOOT_TIME = 5

FUCHSIA_REBOOT_TYPE_SOFT = "soft"
FUCHSIA_REBOOT_TYPE_HARD = "hard"

FUCHSIA_DEFAULT_CONNECT_TIMEOUT = 90
FUCHSIA_DEFAULT_COMMAND_TIMEOUT = 60

FUCHSIA_DEFAULT_CLEAN_UP_COMMAND_TIMEOUT = 15

FUCHSIA_COUNTRY_CODE_TIMEOUT = 15
FUCHSIA_DEFAULT_COUNTRY_CODE_US = "US"

MDNS_LOOKUP_RETRY_MAX = 3

FFX_PROXY_TIMEOUT_SEC = 3

# Duration to wait for the Fuchsia device to acquire an IP address after
# requested to join a network.
#
# Acquiring an IP address after connecting to a WLAN network could take up to
# 15 seconds if we get unlucky:
#
#  1. An outgoing passive scan just started (~7s)
#  2. An active scan is queued for the newly saved network (~7s)
#  3. The initial connection attempt fails (~1s)
IP_ADDRESS_TIMEOUT = 30

_DEVICE_MONITOR_PROXY = FidlEndpoint(
    "core/wlandevicemonitor", "fuchsia.wlan.device.service.DeviceMonitor"
)


class FuchsiaDeviceError(signals.ControllerError):
    pass


class FuchsiaConfigError(signals.ControllerError):
    """Incorrect FuchsiaDevice configuration."""


def create(configs: list[ControllerConfig]) -> list[FuchsiaDevice]:
    return [FuchsiaDevice(c) for c in configs]


def destroy(objects: list[FuchsiaDevice]) -> None:
    for fd in objects:
        fd.clean_up()
        del fd


def get_info(objects: list[FuchsiaDevice]) -> list[Json]:
    """Get information on a list of FuchsiaDevice objects."""
    return [{"ip": fd.ip} for fd in objects]


class FuchsiaDevice:
    """Class representing a Fuchsia device.

    Each object of this class represents one Fuchsia device in ACTS.

    Attributes:
        ip: The full address or Fuchsia abstract name to contact the Fuchsia
            device at
        log: A logger object.
        ssh_port: The SSH TCP port number of the Fuchsia device.
        ssh_config: The ssh_config for connecting to the Fuchsia device.
    """

    def __init__(self, controller_config: ControllerConfig) -> None:
        config = MapValidator(controller_config)
        self.ip = config.get(str, "ip")
        if "%" in self.ip:
            addr, scope_id = self.ip.split("%", 1)
            try:
                if_name = socket.if_indextoname(int(scope_id))
                self.ip = f"{addr}%{if_name}"
            except ValueError:
                # Scope ID is likely already the interface name, no change necessary.
                pass
        self.orig_ip = self.ip
        self.ssh_username = config.get(str, "ssh_username", DEFAULT_SSH_USER)
        self.ssh_port = config.get(int, "ssh_port", DEFAULT_SSH_PORT)
        self.ssh_binary_path = config.get(str, "ssh_binary_path", "ssh")

        def expand(path: str) -> str:
            return os.path.expandvars(os.path.expanduser(path))

        def path_from_config(
            name: str, default: str | None = None
        ) -> str | None:
            path = config.get(str, name, default)
            return None if path is None else expand(path)

        def assert_exists(name: str, path: str | None) -> None:
            if path is None:
                raise FuchsiaDeviceError(
                    f'Please specify "${name}" in your configuration file'
                )
            if not os.path.exists(path):
                raise FuchsiaDeviceError(
                    f'Please specify a correct "${name}" in your configuration '
                    f'file: "{path}" does not exist'
                )

        self.specific_image: str | None = path_from_config("specific_image")
        if self.specific_image:
            assert_exists("specific_image", self.specific_image)

        # Path to a tar.gz archive with pm and amber-files, as necessary for
        # starting a package server.
        self.packages_archive_path: str | None = path_from_config(
            "packages_archive_path"
        )
        if self.packages_archive_path:
            assert_exists("packages_archive_path", self.packages_archive_path)

        def required_path_from_config(
            name: str, default: str | None = None
        ) -> str:
            path = path_from_config(name, default)
            if path is None:
                raise FuchsiaConfigError(f"{name} is a required config field")
            assert_exists(name, path)
            return path

        self.ssh_priv_key: str = required_path_from_config(
            "ssh_priv_key", DEFAULT_SSH_PRIVATE_KEY
        )
        self.ffx_binary_path: str = required_path_from_config(
            "ffx_binary_path", "${FUCHSIA_DIR}/.jiri_root/bin/ffx"
        )
        self.ffx_subtools_search_path: str | None = path_from_config(
            "ffx_subtools_search_path"
        )

        self.authorized_file = config.get(str, "authorized_file_loc", None)
        self.serial_number = config.get(str, "serial_number", None)
        self.device_type = config.get(str, "device_type", None)
        self.product_type = config.get(str, "product_type", None)
        self.board_type = config.get(str, "board_type", None)
        self.build_number = config.get(str, "build_number", None)
        self.build_type = config.get(str, "build_type", None)
        self.name = config.get(str, "name", None)

        self.hard_reboot_on_fail = config.get(
            bool, "hard_reboot_on_fail", False
        )
        self.take_bug_report_on_fail = config.get(
            bool, "take_bug_report_on_fail", False
        )
        self.device_pdu_config = config.get(dict, "PduDevice", {})
        self.config_country_code = config.get(
            str, "country_code", FUCHSIA_DEFAULT_COUNTRY_CODE_US
        ).upper()

        output_path = context.get_current_context().get_base_output_path()
        self.ssh_config = os.path.join(output_path, f"ssh_config_{self.ip}")
        self._generate_ssh_config(self.ssh_config)

        # WLAN interface info is populated inside configure_wlan
        self.wlan_client_interfaces: dict[str, Any] = {}
        self.wlan_ap_interfaces: dict[str, Any] = {}
        self.wlan_client_test_interface_name = config.get(
            str, "wlan_client_test_interface", None
        )
        self.wlan_ap_test_interface_name = config.get(
            str, "wlan_ap_test_interface", None
        )

        # Whether to use 'policy' or 'drivers' for WLAN connect/disconnect calls
        # If set to None, wlan is not configured.
        self.association_mechanism: str | None = None
        # Defaults to policy layer, unless otherwise specified in the config
        self.default_association_mechanism = config.get(
            str, "association_mechanism", "policy"
        )

        if not utils.is_valid_ipv4_address(
            self.ip
        ) and not utils.is_valid_ipv6_address(self.ip):
            mdns_ip = None
            for _ in range(MDNS_LOOKUP_RETRY_MAX):
                mdns_ip = get_fuchsia_mdns_ipv6_address(self.ip)
                if mdns_ip:
                    break
                else:
                    time.sleep(1)
            if mdns_ip and utils.is_valid_ipv6_address(mdns_ip):
                # self.ip was actually an mdns name. Use it for self.name
                # unless one was explicitly provided.
                self.name = self.name or self.ip
                self.ip = mdns_ip
            else:
                raise ValueError(f"Invalid IP: {self.ip}")

        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[FuchsiaDevice | {self.orig_ip}]",
            },
        )

        self.serial = re.sub("[.:%]", "_", self.ip)

        # Create honeydew fuchsia_device.
        if not self.name:
            raise FuchsiaConfigError(
                'Must provide "name: <device mDNS name>" in the device config'
            )

        ffx_config = FfxConfig()
        ffx_config.setup(
            binary_path=self.ffx_binary_path,
            isolate_dir=None,
            logs_dir=f"{getattr(logging, 'log_path')}/ffx/",
            logs_level="Off",
            enable_mdns=False,
            enable_usb=False,
            subtools_search_path=self.ffx_subtools_search_path,
            proxy_timeout_secs=FFX_PROXY_TIMEOUT_SEC,
        )

        self.honeydew_fd = honeydew.create_device(
            device_info=DeviceInfo(
                name=self.name,
                serial_number=self.serial_number,
                ip_port=IpPort(ip_address(self.ip), self.ssh_port),
                serial_socket=None,
            ),
            ffx_config_data=ffx_config.get_config(),
        )

    @cached_property
    def ssh(self) -> FuchsiaSSHProvider:
        """Get the SSH provider module configured for this device."""
        if not self.ssh_port:
            raise FuchsiaConfigError(
                'Must provide "ssh_port: <int>" in the device config'
            )
        if not self.ssh_priv_key:
            raise FuchsiaConfigError(
                'Must provide "ssh_priv_key: <file path>" in the device config'
            )
        return FuchsiaSSHProvider(
            SSHConfig(
                self.ssh_username,
                self.ip,
                self.ssh_priv_key,
                port=self.ssh_port,
                ssh_binary=self.ssh_binary_path,
            )
        )

    @property
    def ffx(self) -> FFX:
        """Returns the underlying Honeydew FFX transport object.

        Returns:
            The underlying Honeydew FFX transport object.

        Raises:
            FfxCommandError: Failed to instantiate.
        """
        return self.honeydew_fd.ffx

    @cached_property
    def wlan_policy_controller(self) -> WlanPolicyController:
        return WlanPolicyController(self.honeydew_fd, self.ssh)

    @cached_property
    def wlan_controller(self) -> WlanController:
        return WlanController(self.honeydew_fd)

    def _generate_ssh_config(self, file_path: str) -> None:
        """Generate and write an SSH config for Fuchsia to disk.

        Args:
            file_path: Path to write the generated SSH config
        """
        content = textwrap.dedent(
            f"""\
            Host *
                CheckHostIP no
                StrictHostKeyChecking no
                ForwardAgent no
                ForwardX11 no
                GSSAPIDelegateCredentials no
                UserKnownHostsFile /dev/null
                User fuchsia
                IdentitiesOnly yes
                IdentityFile {self.ssh_priv_key}
                ControlPersist yes
                ControlMaster auto
                ControlPath /tmp/fuchsia--%r@%h:%p
                ServerAliveInterval 1
                ServerAliveCountMax 1
                LogLevel ERROR
            """
        )

        with open(file_path, "w", encoding="utf-8") as file:
            file.write(content)

    def update_wlan_interfaces(self) -> None:
        """Retrieves WLAN interfaces from device and sets the FuchsiaDevice
        attributes.
        """
        self.wlan_client_interfaces = {}
        self.wlan_ap_interfaces = {}

        import fidl_fuchsia_wlan_common as f_wlan_common
        import fidl_fuchsia_wlan_device_service as f_wlan_device_service

        fc = self.honeydew_fd.fuchsia_controller
        proxy = f_wlan_device_service.DeviceMonitorClient(
            fc.connect_device_proxy(_DEVICE_MONITOR_PROXY)
        )
        loop = fuchsia_async_extension.get_loop()
        ifaces = loop.run_until_complete(proxy.list_ifaces()).iface_list

        wlan_interfaces_by_mac: dict[str, dict[str, int]] = {
            "client": {},
            "ap": {},
        }
        for iface_id in ifaces:
            try:
                resp = (
                    loop.run_until_complete(
                        proxy.query_iface(iface_id=iface_id)
                    )
                    .unwrap()
                    .resp
                )
                mac = "%02x:%02x:%02x:%02x:%02x:%02x" % tuple(resp.sta_addr)
                if resp.role == f_wlan_common.WlanMacRole.CLIENT:
                    wlan_interfaces_by_mac["client"][mac] = iface_id
                elif resp.role == f_wlan_common.WlanMacRole.AP:
                    wlan_interfaces_by_mac["ap"][mac] = iface_id
            except (AssertionError, Exception):
                pass

        netstack_interfaces = (
            fuchsia_async_extension.get_loop().run_until_complete(
                self.honeydew_fd.netstack.list_interfaces()
            )
        )
        for netstack_iface in netstack_interfaces:
            if netstack_iface.mac is None:
                continue
            mac_str = str(netstack_iface.mac)
            if mac_str in wlan_interfaces_by_mac["client"]:
                self.wlan_client_interfaces[
                    netstack_iface.name
                ] = wlan_interfaces_by_mac["client"][mac_str]
            elif mac_str in wlan_interfaces_by_mac["ap"]:
                self.wlan_ap_interfaces[
                    netstack_iface.name
                ] = wlan_interfaces_by_mac["ap"][mac_str]

        # Set test interfaces to value from config, else None
        if self.wlan_client_test_interface_name is None:
            self.wlan_client_test_interface_name = next(
                iter(self.wlan_client_interfaces), None
            )

        if self.wlan_ap_test_interface_name is None:
            self.wlan_ap_test_interface_name = next(
                iter(self.wlan_ap_interfaces), None
            )

    def configure_wlan(
        self,
        association_mechanism: str | None = None,
        clear_networks: bool = True,
    ) -> None:
        """
        Readies device for WLAN functionality. If applicable, connects to the
        policy layer and clears preexisting saved networks.

        Args:
            association_mechanism: either 'policy' or 'drivers'. If None, uses
                the default value from init (can be set by ACTS config)
            clear_networks: whether to clear all saved networks.

        Raises:
            FuchsiaDeviceError, if configuration fails
        """
        self.wlan_controller.set_country_code(
            CountryCode(self.config_country_code)
        )

        # If args aren't provided, use the defaults, which can be set in the
        # config.
        if association_mechanism is None:
            association_mechanism = self.default_association_mechanism

        if association_mechanism not in {None, "policy", "drivers"}:
            raise FuchsiaDeviceError(
                f"Invalid FuchsiaDevice association_mechanism: {association_mechanism}"
            )

        # Allows for wlan to be set up differently in different tests
        if self.association_mechanism:
            self.log.info("Deconfiguring WLAN")
            self.deconfigure_wlan()

        self.association_mechanism = association_mechanism

        self.log.info(
            f"Configuring WLAN w/ association mechanism: {association_mechanism}"
        )
        if association_mechanism == "drivers":
            self.log.warn(
                "You may encounter unusual device behavior when using the "
                "drivers directly for WLAN. This should be reserved for "
                "debugging specific issues. Normal test runs should use the "
                "policy layer."
            )
        else:
            self.wlan_policy_controller.configure_wlan(
                clear_networks=clear_networks
            )

        # Retrieve WLAN client and AP interfaces
        self.update_wlan_interfaces()

    def deconfigure_wlan(self) -> None:
        """
        Stops WLAN functionality (if it has been started). Used to allow
        different tests to use WLAN differently (e.g. some tests require using
        wlan policy, while the abstract wlan_device can be setup to use policy
        or drivers)

        Raises:
            FuchsiaDeviceError, if deconfigure fails.
        """
        if not self.association_mechanism:
            self.log.warning(
                "WLAN not configured before deconfigure was called."
            )
            return
        # If using policy, stop client connections. Otherwise, just clear
        # variables.
        if self.association_mechanism != "drivers":
            self.wlan_policy_controller._deconfigure_wlan()
        self.association_mechanism = None

    def channel_create(self) -> Tuple[Any, Any]:
        """Returns a pair of connected channels.

        Note: this returns the `Any` type, but this is a fuchsia-controller
        channel. This is just an importing issue.
        """
        return self.honeydew_fd.fuchsia_controller.channel_create()

    def reboot(
        self,
        unreachable_timeout: int = FUCHSIA_DEFAULT_CONNECT_TIMEOUT,
        reboot_type: str = FUCHSIA_REBOOT_TYPE_SOFT,
        testbed_pdus: list[pdu.PduDevice] | None = None,
    ) -> None:
        """Reboot a FuchsiaDevice.

        Soft reboots the device, verifies it becomes unreachable, then verifies
        it comes back online. Re-initializes services so the tests can continue.

        Args:
            unreachable_timeout: time to wait for device to become unreachable.
            reboot_type: 'soft' or 'hard'.
            testbed_pdus: all testbed PDUs.

        Raises:
            ConnectionError, if device fails to become unreachable or fails to
                come back up.
        """
        if reboot_type == FUCHSIA_REBOOT_TYPE_SOFT:
            self.log.info("Soft rebooting")
            fuchsia_async_extension.get_loop().run_until_complete(
                self.honeydew_fd.reboot()
            )

        elif reboot_type == FUCHSIA_REBOOT_TYPE_HARD:
            self.log.info("Hard rebooting via PDU")

            # Use dmc (client of DMS, device management server) if available
            # for rebooting the device. This tool is only available when
            # running in Fuchsia infrastructure.
            dmc: PowerSwitchUsingDmc | None = None
            if self.name:
                try:
                    dmc = PowerSwitchUsingDmc(device_name=self.name)
                except PowerSwitchDmcError:
                    self.log.info("dmc not found, falling back to using PDU")

            if dmc:
                self.log.info("Killing power to FuchsiaDevice with dmc")
                self.ffx.notify_intentional_disconnect()
                dmc.power_off()
                self.honeydew_fd.wait_for_offline()

                self.log.info("Restoring power to FuchsiaDevice with dmc")
                dmc.power_on()
                fuchsia_async_extension.get_loop().run_until_complete(
                    self.honeydew_fd.wait_for_online()
                )
                fuchsia_async_extension.get_loop().run_until_complete(
                    self.honeydew_fd.on_device_boot()
                )
            else:
                # Find the matching PDU in the Mobly config.
                if not testbed_pdus:
                    raise AttributeError(
                        "Testbed PDUs must be supplied to hard reboot a fuchsia_device."
                    )
                device_pdu, device_pdu_port = pdu.get_pdu_port_for_device(
                    self.device_pdu_config, testbed_pdus
                )

                self.log.info("Killing power to FuchsiaDevice")
                self.ffx.notify_intentional_disconnect()
                device_pdu.off(device_pdu_port)
                self.honeydew_fd.wait_for_offline()

                self.log.info("Restoring power to FuchsiaDevice")
                device_pdu.on(device_pdu_port)
                fuchsia_async_extension.get_loop().run_until_complete(
                    self.honeydew_fd.wait_for_online()
                )
                fuchsia_async_extension.get_loop().run_until_complete(
                    self.honeydew_fd.on_device_boot()
                )

        else:
            raise ValueError(f"Invalid reboot type: {reboot_type}")

        # Cleanup services
        self.stop_services()

        # TODO(http://b/246852449): Move configure_wlan to other controllers.
        # If wlan was configured before reboot, it must be configured again
        # after rebooting, as it was before reboot. No preserving should occur.
        if self.association_mechanism:
            pre_reboot_association_mechanism = self.association_mechanism
            # Prevent configure_wlan from thinking it needs to deconfigure first
            self.association_mechanism = None
            self.configure_wlan(
                association_mechanism=pre_reboot_association_mechanism,
                clear_networks=False,
            )

        self.log.info("Device has rebooted")

    def clean_up(self) -> None:
        """Cleans up the FuchsiaDevice object, releases any resources it
        claimed, and restores saved networks if applicable. For reboots, use
        clean_up_services only.

        Note: Any exceptions thrown in this method must be caught and handled,
        ensuring that clean_up_services is run. Otherwise, the syslog listening
        thread will never join and will leave tests hanging.
        """
        # If and only if wlan is configured, and using the policy layer
        if self.association_mechanism == "policy":
            try:
                self.wlan_policy_controller.clean_up()
            except Exception as err:
                self.log.warning(f"Unable to clean up WLAN Policy layer: {err}")

        self.stop_services()

    def get_interface_ip_addresses(
        self, interface: str
    ) -> dict[str, list[str]]:
        return get_interface_ip_addresses(self, interface)

    def wait_for_ipv4_addr(self, interface: str) -> None:
        """Checks if device has an ipv4 private address. Sleeps 1 second between
        retries.

        Args:
            interface: name of interface from which to get ipv4 address.

        Raises:
            ConnectionError, if device does not have an ipv4 address after all
            timeout.
        """
        self.log.info(
            f"Checking for valid ipv4 addr. Retry {IP_ADDRESS_TIMEOUT} seconds."
        )
        timeout = time.time() + IP_ADDRESS_TIMEOUT
        while time.time() < timeout:
            ip_addrs = self.get_interface_ip_addresses(interface)

            if len(ip_addrs["ipv4_private"]) > 0:
                self.log.info(
                    f"Device has an ipv4 address: {ip_addrs['ipv4_private'][0]}"
                )
                break
            else:
                self.log.debug(
                    "Device does not yet have an ipv4 address...retrying in 1 second."
                )
                time.sleep(1)
        else:
            raise ConnectionError("Device failed to get an ipv4 address.")

    def wait_for_ipv6_addr(self, interface: str) -> None:
        """Checks if device has an ipv6 private local address. Sleeps 1 second
        between retries.

        Args:
            interface: name of interface from which to get ipv6 address.

        Raises:
            ConnectionError, if device does not have an ipv6 address after all
            timeout.
        """
        self.log.info(
            f"Checking for valid ipv6 addr. Retry {IP_ADDRESS_TIMEOUT} seconds."
        )
        timeout = time.time() + IP_ADDRESS_TIMEOUT
        while time.time() < timeout:
            ip_addrs = self.get_interface_ip_addresses(interface)
            # Check for any valid IPv6 address. While the test ideally uses a
            # routed ULA (private local) address, a link-local address is
            # sufficient for connectivity in some environments.
            valid_addrs = (
                ip_addrs["ipv6_link_local"]
                + ip_addrs["ipv6_private_local"]
                + ip_addrs["ipv6_public"]
            )
            if len(valid_addrs) > 0:
                self.log.info(f"Device has valid ipv6 addresses: {valid_addrs}")
                break
            else:
                self.log.debug(
                    "Device does not yet have an ipv6 address...retrying in 1 second."
                )
                time.sleep(1)
        else:
            raise ConnectionError("Device failed to get an ipv6 address.")

    def stop_services(self) -> None:
        """Stops all host-side clients to the Fuchsia device.

        This is necessary whenever the device's state is unknown. These cases can be
        found after device reboots, for example.
        """
        self.log.info("Stopping host device services.")
        del self.wlan_policy_controller
        del self.wlan_controller
        del self.ssh

    def take_bug_report(self) -> None:
        """Takes a bug report on the device and stores it in a file."""
        self.log.info(f"Taking snapshot of {self.name}")

        time_stamp = logger.sanitize_filename(
            logger.epoch_to_log_line_timestamp(utils.get_current_epoch_time())
        )
        out_dir = context.get_current_context().get_full_output_path()
        out_path = os.path.join(out_dir, f"{self.name}_{time_stamp}.zip")

        try:
            with open(out_path, "wb") as file:
                snapshot_bytes = self.ssh.run(
                    "snapshot", log_output=False
                ).stdout
                file.write(snapshot_bytes)
            self.log.info(f"Snapshot saved to {out_path}")
        except Exception as err:
            self.log.error(f"Failed to take snapshot: {err}")

    def take_bt_snoop_log(self, custom_name: str | None = None) -> None:
        """Takes a the bt-snoop log from the device and stores it in a file
        in a pcap format.
        """
        bt_snoop_path = context.get_current_context().get_full_output_path()
        time_stamp = logger.sanitize_filename(
            logger.epoch_to_log_line_timestamp(time.time())
        )
        out_name = "FuchsiaDevice%s_%s" % (
            self.serial,
            time_stamp.replace(" ", "_").replace(":", "-"),
        )
        out_name = f"{out_name}.pcap"
        if custom_name:
            out_name = f"{self.serial}_{custom_name}.pcap"
        else:
            out_name = f"{out_name}.pcap"
        full_out_path = os.path.join(bt_snoop_path, out_name)
        with open(full_out_path, "wb") as file:
            pcap_bytes = self.ssh.run("bt-snoop-cli -d -f pcap").stdout
            file.write(pcap_bytes)
