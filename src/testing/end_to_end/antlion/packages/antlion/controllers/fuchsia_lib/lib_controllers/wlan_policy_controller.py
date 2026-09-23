#!/usr/bin/env python3
#
# Copyright 2025 The Fuchsia Authors
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import logging
import time
from dataclasses import dataclass
from datetime import timedelta

import fidl_fuchsia_wlan_policy as f_wlan_policy
import fuchsia_async_extension
from antlion.controllers.fuchsia_lib.ssh import FuchsiaSSHProvider
from honeydew.affordances.connectivity.wlan.utils.errors import (
    HoneydewWlanError,
)
from honeydew.affordances.connectivity.wlan.utils.types import NetworkState
from honeydew.fuchsia_device.fuchsia_device import (
    FuchsiaDevice as HdFuchsiaDevice,
)
from mobly import logger, signals

SESSION_MANAGER_TIMEOUT_SEC = 10
FUCHSIA_DEFAULT_WLAN_CONFIGURE_RETRIES = 3
DEFAULT_GET_UPDATE_TIMEOUT = 60
_DEFAULT_TIME_WAIT_FOR_CLIENT_CONNECTIONS_STATE = timedelta(seconds=5)
TIME_WAIT_BETWEEN_STOP_START_CONNECTIONS = 0.15  # 150 ms in seconds for "sleep"


class WlanPolicyControllerError(signals.ControllerError):
    pass


@dataclass
class ClientState:
    state: str
    networks: list[dict[str, object]]


# TODO(http://b/309854439): Add a ClientStateWatcher and refactor tests to allow test
# developers more control when update listeners are set and the client update state is
# reset.
class WlanPolicyController:
    """Contains methods related to the wlan policy layer, to be used in the
    FuchsiaDevice object."""

    def __init__(
        self, honeydew: HdFuchsiaDevice, ssh: FuchsiaSSHProvider
    ) -> None:
        self.policy_configured = False
        self.honeydew = honeydew
        self.ssh = ssh
        self.log = logger.PrefixLoggerAdapter(
            logging.getLogger(),
            {
                logger.PrefixLoggerAdapter.EXTRA_KEY_LOG_PREFIX: f"[WlanPolicyController | {self.ssh.config.host_name}]",
            },
        )

    def configure_wlan(
        self,
        restart_client_connections: bool = True,
        clear_networks: bool = True,
        retries: int = FUCHSIA_DEFAULT_WLAN_CONFIGURE_RETRIES,
    ) -> None:
        """Sets up wlan policy layer.

        Args:
            restart_client_connections: whether to restart client connections.
            clear_networks: whether to clear all saved networks.
            retries: number of times to re-attempt to configure WLAN policy.
        """

        # We need to stop session manager to free control of
        # fuchsia.wlan.policy.ClientController, which can only be used by a
        # single caller at a time. Fuchsia Controller needs the ClientController
        # to trigger WLAN policy state changes. On eng builds the
        # session_manager can be restarted after being stopped during reboot so
        # we attempt killing the session manager process for 10 seconds.
        # See https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.policy/client_provider.fidl
        if b"cast_agent.cm" in self.ssh.run("ps").stdout:
            session_manager_expiration = (
                time.time() + SESSION_MANAGER_TIMEOUT_SEC
            )
            while time.time() < session_manager_expiration:
                self.ssh.stop_component(
                    "session_manager", is_cfv2_component=True
                )

        # Acquire control of policy layer
        for attempt in range(retries):
            try:
                if clear_networks:
                    self.log.info(
                        "Removing any and all saved networks to run tests in a clean state."
                    )
                    fuchsia_async_extension.get_loop().run_until_complete(
                        self.honeydew.wlan_policy.remove_all_networks()
                    )

                # Optionally restart client connections to start tests in a good state. This should
                # prevent issues like scans still being in progress when tests start.
                if restart_client_connections:
                    self.log.info(
                        "Restarting client connections to run test in a clean state"
                    )
                    self.stop_client_connections_and_wait()

                    # Sleep 150 ms to give a bit of time and specifically to prevent race
                    # conditions with retrying failed scans
                    time.sleep(TIME_WAIT_BETWEEN_STOP_START_CONNECTIONS)

                fuchsia_async_extension.get_loop().run_until_complete(
                    self.honeydew.wlan_policy.start_client_connections()
                )
                self.log.info(
                    "ACTS tests now have control of the WLAN policy layer."
                )
                self.policy_configured = True
                return
            except HoneydewWlanError as e:
                self.log.warning(
                    "Attempt %d/%d to configure WLAN policy failed: %s",
                    attempt + 1,
                    retries,
                    e,
                )
            except TimeoutError as e:
                raise WlanPolicyControllerError(
                    f"An operation timed out while configuring WLAN policy client controller: {e}."
                )
        raise WlanPolicyControllerError(
            f"Failed to configure WLAN policy client controller after {retries} retries."
        )

    def _deconfigure_wlan(self) -> None:
        fuchsia_async_extension.get_loop().run_until_complete(
            self.honeydew.wlan_policy.stop_client_connections()
        )
        self.policy_configured = False

    def stop_client_connections_and_wait(
        self,
        wait_time: timedelta = _DEFAULT_TIME_WAIT_FOR_CLIENT_CONNECTIONS_STATE,
    ) -> None:
        """This function stops client connections if client connections are currently enabled,
        and waits for an update showing that the state has changed.
        """
        try:
            client = fuchsia_async_extension.get_loop().run_until_complete(
                self.honeydew.wlan_policy.get_status(
                    timeout=wait_time.total_seconds()
                )
            )
            if (
                client.state
                != f_wlan_policy.WlanClientState.CONNECTIONS_ENABLED
            ):
                self.log.info(
                    "Client connections are not enabled, so they will not be disabled."
                )
                return
        except TimeoutError:
            # This update should basically be immediate because we get a new client state listener
            # channel, so this is unexpected
            self.log.warning(
                "Unexpectedly timed out getting client state. Proceeding to stop client connections"
            )

        fuchsia_async_extension.get_loop().run_until_complete(
            self.honeydew.wlan_policy.stop_client_connections()
        )
        try:
            fuchsia_async_extension.get_loop().run_until_complete(
                self.honeydew.wlan_policy.wait_for_client_state(
                    expected_state=f_wlan_policy.WlanClientState.CONNECTIONS_DISABLED,
                    timeout=wait_time,
                )
            )
        except TimeoutError:
            self.log.warning(
                "Timed out waiting for client connections disabled update. "
                "Will proceed with the test as normal anyway."
            )

    def clean_up(self) -> None:
        # It is possible for policy to have been configured before, but
        # deconfigured before test end. In this case, in must be setup
        # before removing networks. This will both set up the DUT and remove
        # networks.
        if not self.policy_configured:
            self.configure_wlan(
                clear_networks=True, restart_client_connections=False
            )

    def _find_network(
        self,
        ssid: str,
        networks: list[NetworkState],
    ) -> NetworkState | None:
        """Helper method to find network in list of network states.

        Args:
            ssid: The network name to look for.
            networks: The list of network states to look in.

        Returns:
            Network state of target ssid or None if not found in networks.
        """
        for network in networks:
            if network.network_identifier.ssid == ssid:
                return network
        return None
