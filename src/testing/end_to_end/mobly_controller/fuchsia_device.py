# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly Controller for Fuchsia Device"""

import logging
import os
import tempfile
from copy import deepcopy
from typing import Any

import honeydew
from honeydew.fuchsia_device.fuchsia_device import FuchsiaDevice
from honeydew.transports.ffx import config as ffx_config
from honeydew.typing import custom_types
from honeydew.utils import properties

MOBLY_CONTROLLER_CONFIG_NAME = "FuchsiaDevice"

_LOGGER: logging.Logger = logging.getLogger(__name__)

_FFX_CONFIG_OBJ: ffx_config.FfxConfig = ffx_config.FfxConfig()

_FFX_CONFIG_LOGS_LEVEL: str = "debug"

_FFX_CONFIG_PROXY_TIMEOUT_SECS: int = 30


async def create(
    configs: list[dict[str, Any]],
) -> list[FuchsiaDevice]:
    """Create Fuchsia device controller(s) and returns them.

    Required for Mobly controller registration.

    Args:
        configs: list of dicts. Each dict representing a configuration for a
            Fuchsia device.

    Returns:
        A list of FuchsiaDevice objects.
    """
    _LOGGER.debug(
        "FuchsiaDevice controller configs received in testbed yml file is '%s'",
        configs,
    )

    test_logs_dir: str = _get_log_directory()
    ffx_config_dict: dict[str, Any] = _get_ffx_config(configs)
    ffx_config_dict["emu_instance_dir"] = os.path.join(test_logs_dir, "emu")

    device_configs = []
    ssh_private_keys = []
    for config in configs:
        device_config: dict[str, Any] = _parse_device_config(config)
        device_configs.append(device_config)
        if (
            "ssh_key" in device_config
            and device_config["ssh_key"] not in ssh_private_keys
        ):
            ssh_private_keys.append(device_config["ssh_key"])

    # If no SSH private keys are explicitly provided in the Mobly config,
    # resolve default SSH keys from the environment or fallback paths.
    # This is required to override strict-mode ffx configuration defaults.
    if not ssh_private_keys:
        env_ssh_key = os.environ.get("FUCHSIA_SSH_KEY")
        if env_ssh_key:
            ssh_private_keys.append(env_ssh_key)
        else:
            default_priv_key = os.path.expanduser("~/.ssh/fuchsia_ed25519")
            if os.path.exists(default_priv_key):
                ssh_private_keys.append(default_priv_key)

    # Call `FfxConfig.setup` before calling `create_device` as
    # `create_device` results in calling an FFX command and we
    # don't want to miss those FFX logs

    # Note - As of now same FFX Config is used across all fuchsia devices.
    # This means we will use a single FFX configuration to interact with all
    # fuchsia devices in the testbed.
    # This is okay for in-tree use cases but may not work for OOT cases where
    # each fuchsia device may be running different build that require different
    # FFX version.
    # This will also not work if you have 2 devices in testbed with one uses
    # device_ip and one uses device_name for FFX commands. This should not
    # happen in our setups though as we will either have mdns enabled or
    # disabled on host.
    # Right fix for all such cases is to use separate ffx config per device.
    # This support will be added when needed in future.
    _FFX_CONFIG_OBJ.setup(
        binary_path=ffx_config_dict["path"],
        isolate_dir=None,
        logs_dir=f"{test_logs_dir}/ffx/",
        logs_level=ffx_config_dict["logs_level"],
        enable_mdns=ffx_config_dict.get("enable_mdns", True),
        subtools_search_path=ffx_config_dict.get("subtools_search_path"),
        proxy_timeout_secs=ffx_config_dict["proxy_timeout_secs"],
        ssh_keepalive_timeout=ffx_config_dict.get("ssh_keepalive_timeout"),
        enable_usb=ffx_config_dict["enable_usb"],
        usb_socket_path=ffx_config_dict.get("usb_socket_path"),
        usb_driver_autostart=ffx_config_dict.get("usb_driver_autostart")
        or False,
        emu_instance_dir=ffx_config_dict.get("emu_instance_dir"),
        ssh_private_keys=ssh_private_keys if ssh_private_keys else None,
        ssh_auth_sock=ffx_config_dict.get("ssh_auth_sock"),
        identities_only=ffx_config_dict.get("identities_only"),
    )

    fuchsia_devices = []
    for device_config in device_configs:
        fuchsia_devices.append(
            honeydew.create_device(
                device_info=custom_types.DeviceInfo(
                    name=device_config["name"],
                    serial_number=device_config.get("device_serial"),
                    ip_port=device_config.get("device_ip_port"),
                    serial_socket=device_config.get("serial_socket"),
                    fastboot_node_id=device_config.get("fastboot_sernum")
                    or device_config.get("fastboot_node_id"),
                ),
                ffx_config_data=_FFX_CONFIG_OBJ.get_config(),
                config=device_config["honeydew_config"],
            )
        )
    return fuchsia_devices


async def destroy(
    fuchsia_devices: list[FuchsiaDevice],
) -> None:
    """Closes all created fuchsia devices.

    Required for Mobly controller registration.

    Args:
        fuchsia_devices: A list of FuchsiaDevice objects.
    """
    for fuchsia_device in fuchsia_devices:
        await fuchsia_device.close()

    # TODO: b/552006166 - Remove this method and other isolate dir cleanup,
    # as it is no longer necessary with `ffx --strict`.
    # Call `FfxConfig.close` manually even though it's already registered for
    # clean up in `FfxConfig.setup` in order to minimize chance of FFX resource
    # leak in the event that SIGKILL/SIGTERM is received between `destroy` and
    # test program exit.
    _FFX_CONFIG_OBJ.close()


async def get_info(
    fuchsia_devices: list[FuchsiaDevice],
) -> list[dict[str, Any]]:
    """Gets information from a list of FuchsiaDevice objects.

    Optional for Mobly controller registration.

    Args:
        fuchsia_devices: A list of FuchsiaDevice objects.

    Returns:
        A list of dict, each representing info for an FuchsiaDevice objects.
    """
    return [
        await _get_fuchsia_device_info(fuchsia_device)
        for fuchsia_device in fuchsia_devices
    ]


async def _get_fuchsia_device_info(
    fuchsia_device: FuchsiaDevice,
) -> dict[str, Any]:
    """Returns information of a specific fuchsia device object.

    Args:
        fuchsia_device: FuchsiaDevice object.

    Returns:
        dict containing information of a fuchsia device.
    """
    _LOGGER.debug("Getting the device info for %s", fuchsia_device.device_name)
    device_info: dict[str, Any] = {
        "device_class": fuchsia_device.__class__.__name__,
        "persistent": {},
        "dynamic": {},
    }

    for attr in dir(fuchsia_device):
        if attr.startswith("_"):
            continue

        try:
            attr_type: Any = getattr(type(fuchsia_device), attr, None)
            if isinstance(attr_type, properties.DynamicProperty):
                _LOGGER.debug("Reading dynamic device property: %s", attr)
                device_info["dynamic"][attr] = getattr(fuchsia_device, attr)
            elif isinstance(attr_type, properties.PersistentProperty):
                _LOGGER.debug("Reading persistent device property: %s", attr)
                device_info["persistent"][attr] = getattr(fuchsia_device, attr)
        except NotImplementedError:
            pass

    _LOGGER.debug("Device info complete for %s", fuchsia_device.device_name)
    return device_info


# LINT.IfChange
def _parse_device_config(config: dict[str, str]) -> dict[str, Any]:
    """Validates and parses mobly configuration associated with FuchsiaDevice.

    Args:
        config: The mobly configuration associated with FuchsiaDevice.

    Returns:
        Validated and parsed mobly configuration associated with FuchsiaDevice.

    Raises:
        RuntimeError: If the fuchsia device name in the config is missing.
        ValueError: If either transport device_ip_port is invalid.
    """
    _LOGGER.debug(
        "FuchsiaDevice controller config received in testbed yml file is '%s'",
        config,
    )

    # Sample testbed file format for FuchsiaDevice controller used in infra...
    # - Controllers:
    #     FuchsiaDevice:
    #     - ipv4: ''
    #       ipv6: fe80::93e3:e3d4:b314:6e9b%qemu
    #       nodename: botanist-target-qemu
    #       serial_socket: ''
    #       ssh_key: private_key
    #       device_ip_port: [::1]:8022
    if "name" not in config:
        raise RuntimeError("Missing fuchsia device name in the config")

    if "honeydew_config" not in config:
        raise RuntimeError("Missing Honeydew config field in the config")

    device_config: dict[str, Any] = {}

    for config_key, config_value in config.items():
        if config_key == "device_ip_port":
            try:
                device_config[
                    "device_ip_port"
                ] = custom_types.IpPort.create_using_ip_and_port(config_value)
            except Exception as err:  # pylint: disable=broad-except
                raise ValueError(
                    f"Invalid device_ip_port `{config_value}` passed for "
                    f"{config['name']}"
                ) from err
        elif config_key in ["ipv4", "ipv6"]:
            if config.get("ipv4"):
                device_config[
                    "device_ip_port"
                ] = custom_types.IpPort.create_using_ip(config["ipv4"])
            if config.get("ipv6"):
                device_config[
                    "device_ip_port"
                ] = custom_types.IpPort.create_using_ip(config["ipv6"])
        else:
            device_config[config_key] = config_value

    _LOGGER.debug(
        "Updated FuchsiaDevice controller config after the validation is '%s'",
        device_config,
    )

    return device_config


# LINT.ThenChange(//build/bazel_sdk/bazel_rules_fuchsia/fuchsia/tools/run_lacewing_test.py)


def _get_log_directory() -> str:
    """Returns the path to the directory where logs should be stored.

    Returns:
        Directory path.
    """
    # TODO(https://fxbug.dev/42078903): Read log path from config once this issue is fixed
    return getattr(
        logging,
        "log_path",  # Set by Mobly in base_test.BaseTestClass.run.
        tempfile.mkdtemp(
            prefix="fuchsia_test_logs_"
        ),  # Safe fallback if run outside Mobly (e.g. standalone/tests)
    )


def _get_ffx_config(configs: list[dict[str, Any]]) -> dict[str, Any]:
    """Read the FFX configuration from the FuchsiaDevice config and validate the mandatory config.

    Args:
      configs: list of dicts. Each dict representing a configuration for a
            Fuchsia device.

    Returns:
        Dict pointing to FFX configuration.
    """
    ffx_config_dict: dict[str, Any] = {}

    # FFX config is currently global and not localized to the individual devices so
    # just return the the first ffx config encountered.
    for config in configs:
        try:
            ffx_config_dict = deepcopy(
                config["honeydew_config"]["transports"]["ffx"]
            )
        except KeyError:
            pass

    if not ffx_config_dict:
        raise RuntimeError("No FFX config found in any of the device config")

    if "path" not in ffx_config_dict:
        raise RuntimeError("No FFX path found in device config")

    if "logs_level" not in ffx_config_dict:
        ffx_config_dict["logs_level"] = _FFX_CONFIG_LOGS_LEVEL

    if "proxy_timeout_secs" not in ffx_config_dict:
        ffx_config_dict["proxy_timeout_secs"] = _FFX_CONFIG_PROXY_TIMEOUT_SECS

    for ffx_config_key in ["proxy_timeout_secs", "ssh_keepalive_timeout"]:
        if ffx_config_key in ffx_config_dict:
            try:
                ffx_config_dict[ffx_config_key] = int(
                    ffx_config_dict[ffx_config_key]
                )
            except (TypeError, ValueError) as err:
                raise RuntimeError(
                    f"Invalid value sent in '{ffx_config_key}'. Please pass a int value"
                ) from err
    ffx_config_dict["enable_usb"] = True

    ssh_auth_sock = ffx_config_dict.get("ssh_auth_sock") or ffx_config_dict.get(
        "ssh.auth-sock"
    )
    if not ssh_auth_sock:
        ssh_auth_sock = os.environ.get("SSH_AUTH_SOCK")
    if ssh_auth_sock:
        ffx_config_dict["ssh_auth_sock"] = ssh_auth_sock

    return ffx_config_dict
