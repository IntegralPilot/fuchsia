# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Contains all Mobly APIs used in Mobly Driver."""

import os
from typing import Any

from mobly import keys, records

from . import api_infra

LATEST_RES_SYMLINK_NAME: str = "latest"

# Fuchsia-specific keys and values used in Mobly configs.
# Defined and used in
# https://osscs.corp.google.com/fuchsia/fuchsia/+/main:src/testing/end_to_end/mobly_controller/fuchsia_device.py
MOBLY_CONTROLLER_FUCHSIA_DEVICE: str = "FuchsiaDevice"
SSH_PATH_KEY: str = "ssh_binary_path"
SSH_CONFIG_KEY: str = "ssh_config"
SSH_HOST_KEY: str = "host"
SSH_USER_KEY: str = "user"
SSH_IDENTITY_FILE_KEY: str = "identity_file"
HONEYDEW_CONFIG_KEY: str = "honeydew_config"
ALLOW_REGDB_BYPASS: str = "allow_regdb_bypass"


MoblyConfigComponent = dict[str, Any]


class ApiException(Exception):
    pass


def get_latest_test_output_dir_symlink_path(
    mobly_output_path: str, testbed_name: str
) -> str:
    """Returns the absolute path to the Mobly testbed's latest output directory.

    Args:
        mobly_output_path: absolute path to Mobly's top-level output directory.
        testbed_name: Mobly testbed name that corresponds to the test output.

    Raises:
      ApiException if arguments are invalid.

    Returns:
      The absolute path to a Mobly testbed's test output directory.
    """
    if not mobly_output_path or not testbed_name:
        raise ApiException("Arguments must be non-empty.")
    return os.path.join(
        mobly_output_path, testbed_name, LATEST_RES_SYMLINK_NAME
    )


def get_result_path(mobly_output_path: str, testbed_name: str) -> str:
    """Returns the absolute path to the Mobly test result file.

    Args:
        mobly_output_path: absolute path to Mobly's top-level output directory.
        testbed_name: Mobly testbed name that corresponds to the test output.

    Raises:
      ApiException if arguments are invalid.

    Returns:
      The absolute path to a Mobly test result file.
    """
    if not mobly_output_path or not testbed_name:
        raise ApiException("Arguments must be non-empty.")
    return os.path.join(
        get_latest_test_output_dir_symlink_path(
            mobly_output_path, testbed_name
        ),
        records.OUTPUT_FILE_SUMMARY,
    )


# TODO(https://fxbug.dev/42070262) - Update |controllers| type to use Honeydew's
# definition. When Honeydew's Mobly device class is available, we
# should use that class as the Pytype to reduce the chance of controller
# instantiation error.
def new_testbed_config(
    testbed_name: str,
    output_path: str,
    honeydew_config: dict[str, Any],
    mobly_controllers: list[dict[str, Any]],
    test_params_dict: MoblyConfigComponent,
    botanist_honeydew_map: dict[str, str],
    ssh_path: str | None = None,
) -> MoblyConfigComponent:
    """Returns a Mobly testbed config which is required for running Mobly tests.

        This method expects the |controller| object to follow the schema of
        tools/botanist/targets/target.go's |FuchsiaTestbedConfig| struct or the
        tools/botanist/targets/auxiliary.go's |Auxiliary| struct.

    # LINT.IfChange
    # Example |mobly_controllers|:
    #    [{
    #       "type": "FuchsiaDevice",
    #       "nodename":"fuchsia-54b2-030e-eb19",
    #       "ipv4":"192.168.42.112",
    #       "ipv6":"",
    #       "fastboot_sernum":"3676a5a3",
    #       "serial_socket":"/tmp/fuchsia-54b2-030e-eb19_mux",
    #       "ssh_key":"/etc/botanist/keys/pkey_infra",
    #       "shared_data":"/etc/botanist/shared_data"
    #    },
    # LINT.ThenChange(//tools/botanist/targets/target.go)
           {
              "type": "AccessPoint",
              "ip": "192.168.42.11",
              "mac": "98:de:d0:81:cf:30",
              "pdu": {
                  "ip": "192.168.42.23",
                  "mac": "0c:73:eb:b0:86:fa",
                  "port": 1
              },
              "nodename": "access-point-1234-5678-9abc",
              "ssh_key": "/etc/botanist/keys/chrome_os_testing_rsa",
              "user": "root",
              "wan_interface": "eth0"
            }]

        Example output:
           {
              "TestBeds": [
                {
                  "Name": "LocalTestbed",
                  "Controllers": {
                    "FuchsiaDevice": [
                      {
                        "name":"fuchsia-54b2-030e-eb19",
                        "ipv4":"192.168.42.112",
                        "ipv6":"",
                        "serial_socket":"/tmp/fuchsia-54b2-030e-eb19_mux",
                        "config": {
                          "transports": {
                            "ffx": {
                              "path":"/path/to/ffx",
                              "subtools_search_path":"/path/to/ffx/subtools",
                              "shared_data":"/etc/botanist/shared_data"
                            }
                          }
                        }
                      }
                    ],
                    "AccessPoint": [
                      {
                        "nodename": "access-point-1234-5678-9abc",
                        "wan_interface": "eth0",
                        "ssh_config": {
                          "ssh_binary_path": host_x64/test_data/src/testing/end_to_end/antlion/tests/wlan_policy/hidden_networks_test/ssh
                          "host": "192.168.42.11",
                          "user": "root",
                          "identity_file": "/etc/botanist/keys/chrome_os_testing_rsa"
                        },
                        "PduDevice" : {
                          "device": synaccess.np02b,
                          "host": "192.168.42.33",
                          "port": 1
                        },
                      }
                    ]
                  },
                  "TestParams": {
                    "test_dir": "/tmp/out"
                  }
                }
              ]
            }

        Args:
            testbed_name: Mobly testbed name to use.
            output_path: absolute path to Mobly's top-level output directory.
            honeydew_config: Honeydew configuration.
            mobly_controllers: List of Mobly controller objects.
            test_params_dict: Mobly testbed params dictionary.
            botanist_honeydew_map: Dictionary that maps Botanist config names to
                                   Honeydew config names.
            ssh_path: absolute path to the SSH binary or None for local test case.
        Returns:
          A Mobly Config that corresponds to the user-specified arguments.
    """
    controllers: dict[str, list[dict[str, Any]]] = {}
    for controller in mobly_controllers:
        controller_type = controller["type"]
        del controller["type"]
        if api_infra.FUCHSIA_DEVICE == controller_type:
            # Add the "honeydew_config" field for every Fuchsia device, if exists.
            controller[HONEYDEW_CONFIG_KEY] = honeydew_config

            # Propagate shared_data to the ffx transport config
            if "shared_data" in controller:
                shared_data = controller.pop("shared_data")

                # If shared_data doesn't exist, check if its nearest existing parent is writable.
                # If it exists, check if it is writable.
                path_to_check = shared_data
                while path_to_check and not os.path.exists(path_to_check):
                    path_to_check = os.path.dirname(path_to_check)

                is_writable = path_to_check and os.access(
                    path_to_check, os.W_OK
                )

                if not is_writable:
                    original_shared_data = shared_data
                    output_path_parent = output_path
                    while output_path_parent and not os.path.exists(
                        output_path_parent
                    ):
                        output_path_parent = os.path.dirname(output_path_parent)
                    if output_path_parent and os.access(
                        output_path_parent, os.W_OK
                    ):
                        shared_data = os.path.join(output_path, "shared_data")
                    else:
                        shared_data = "/tmp/shared_data"
                    print(
                        f"Warning: Provided shared_data directory "
                        f"'{original_shared_data}' is not writable. "
                        f"Using '{shared_data}' instead."
                    )

                honeydew_config = controller[HONEYDEW_CONFIG_KEY]
                transports = honeydew_config.setdefault("transports", {})
                ffx = transports.setdefault("ffx", {})
                ffx["shared_data"] = shared_data

            # Convert botanist key names to relative Honeydew key names for
            # fuchsia devices. This is done here so that Honeydew does not have
            # to do the conversions itself.
            for botanist_key, honeydew_key in botanist_honeydew_map.items():
                if botanist_key in controller:
                    controller[honeydew_key] = controller.pop(botanist_key)
        elif api_infra.ACCESS_POINT == controller_type:
            model = controller.pop("model", None)
            if model == "OpenWrtOne":
                controller_type = api_infra.OPENWRT_AP

            controller.setdefault(
                ALLOW_REGDB_BYPASS,
                test_params_dict.get(ALLOW_REGDB_BYPASS, False),
            )

            controller[SSH_CONFIG_KEY] = {
                SSH_HOST_KEY: controller.pop("ip"),
                SSH_USER_KEY: controller.pop("user"),
                SSH_IDENTITY_FILE_KEY: controller.pop("ssh_key"),
            }
            if ssh_path:
                controller[SSH_CONFIG_KEY][SSH_PATH_KEY] = ssh_path

        if controller_type in controllers:
            controllers[controller_type].append(controller)
        else:
            controllers[controller_type] = [controller]

    config_dict = {
        keys.Config.key_testbed.value: [
            {
                keys.Config.key_testbed_name.value: testbed_name,
                keys.Config.key_testbed_controllers.value: controllers,
            },
        ],
        keys.Config.key_mobly_params.value: {
            keys.Config.key_log_path.value: output_path
        },
    }

    return get_config_with_test_params(config_dict, test_params_dict)


def get_config_with_test_params(
    config_dict: MoblyConfigComponent, params_dict: MoblyConfigComponent
) -> MoblyConfigComponent:
    """Returns a Mobly config with a populated 'TestParams' field.

    Replaces the field if it already exists.

    Args:
        config_dict: The Mobly config dictionary to update.
        params_dict: The Mobly testbed params dictionary to add to the config.

    Returns:
      A MoblyConfigComponent object.

    Raises:
      ApiException if |config_dict| is invalid.
    """
    try:
        ret = config_dict.copy()
        for tb in ret[keys.Config.key_testbed.value]:
            tb[keys.Config.key_testbed_test_params.value] = params_dict
        return ret
    except (AttributeError, KeyError, TypeError) as e:
        raise ApiException("Unexpected Mobly config content: %s" % e)


def set_honeydew_config(
    mobly_config: MoblyConfigComponent, honeydew_config: dict[str, Any]
) -> None:
    """Updates all fuchsia device configs to use the specified honeydew config.

    Overwrites the existing value if the key already exists.

    Args:
      mobly_config: Mobly config object to update.
      honeydew_config: Honeydew config to set on fuchsia devices in the Mobly config.
    """
    _set_per_device_config(mobly_config, HONEYDEW_CONFIG_KEY, honeydew_config)


def _set_per_device_config(
    mobly_config: MoblyConfigComponent, key: str, value: object
) -> None:
    """Updates all fuchsia device configs to contain a key-value pair.

    Overwrites the existing value if the key already exists.

    Args:
      mobly_config: Mobly config object to update.
      key: Device config key to update.
      value: Config value to use.
    """
    for testbed in mobly_config.get(keys.Config.key_testbed.value, []):
        controllers = testbed.get(keys.Config.key_testbed_controllers.value, {})
        for device in controllers.get(MOBLY_CONTROLLER_FUCHSIA_DEVICE, []):
            device[key] = value
