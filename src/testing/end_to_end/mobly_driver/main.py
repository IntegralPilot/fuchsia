#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Entry point of Mobly Driver which conducts Mobly test execution."""

import argparse
import json
import logging
import os
import sys
import traceback
from datetime import timedelta
from typing import Any

import logging_utils
import mobly_driver
from mobly_driver import driver_factory

parser = argparse.ArgumentParser()
parser.add_argument(
    "mobly_test_path",
    help="path to the Mobly test archive produced by the GN build system.",
)
parser.add_argument(
    "--config-yaml-path",
    default=None,
    help="path to the Mobly test config YAML file.",
)
parser.add_argument(
    "--params-yaml-path",
    default=None,
    help="path to the Mobly test params YAML file.",
)
parser.add_argument(
    "--honeydew-config-json-path",
    default=None,
    help="path to the Honeydew config json file.",
)
parser.add_argument(
    "--test-timeout-sec",
    default=None,
    type=int,
    help="integer to specify number of seconds before a Mobly test times out.",
)
parser.add_argument(
    "--cleanup-period-sec",
    default=None,
    type=int,
    help="if set, we send SIGTERM to the test this many seconds before the test times out.",
)
parser.add_argument("--ffx-path", default=None, help="path to FFX.")
parser.add_argument(
    "--ssh-path",
    default=None,
    help="path to SSH binary used by test connectivity labs access points.",
)
parser.add_argument(
    "--ffx-subtools-path",
    default=None,
    help="path to FFX subtools path.",
)
parser.add_argument(
    "--multi-device",
    action="store_const",
    const=True,
    default=False,
    help="Whether the mobly test requires 2+ Fuchsia devices to run.",
)
parser.add_argument(
    "--hermetic",
    action="store_const",
    const=True,
    default=False,
    help="Whether the mobly test is a self-contained executable.",
)
parser.add_argument(
    "--target-address-type",
    default=None,
    help="Whether to use the fuchsia device's name, ip, or id (serial number) for host-target interactions when using FFX and Fuchsia-Controller transports.",
)
parser.add_argument(
    "--ssh-key",
    default=None,
    help="Path to the SSH key for the fuchsia device.",
)
parser.add_argument(
    "--ssh-auth-sock",
    default=None,
    help="Path to the SSH authentication socket (SSH_AUTH_SOCK) to use for FFX/SSH.",
)
parser.add_argument(
    "--ssh-identities-only",
    action="store_true",
    default=None,
    help="When set, tells OpenSSH to only offer explicit identity keys (-i) and ignore other ssh-agent identities.",
)
parser.add_argument(
    "--ap-ip",
    default=None,
    help="IP address of the access point.",
)
parser.add_argument(
    "--ap-ssh-port",
    default=None,
    type=int,
    help="SSH port of the access point.",
)
parser.add_argument(
    "--ap-ssh-key",
    default=None,
    help="Path to the SSH key for the access point.",
)

parser.add_argument(
    "--v",
    action="store_const",
    const=True,
    default=False,
    help="run the mobly test with the --v flag.",
)
parser.add_argument(
    "--test_cases",
    nargs="*",
    default=[],
    type=str,
    help="List of test cases to run.",
)
parser.add_argument(
    "--list_mobly_tests",
    action="store_true",
    default=False,
    help="List test cases.",
)
args = parser.parse_args()


def main() -> None:
    """Executes the Mobly test via Mobly Driver.

    This function determines the appropriate Mobly Driver implementation to use
    based on the execution environment, and uses the Mobly Driver to run the
    underlying Mobly test.
    """
    handler = logging.StreamHandler(sys.stdout)
    handler.setFormatter(
        logging_utils.ColoredFormatter(
            "[Mobly Driver] %(asctime)s %(levelname)s %(message)s",
        )
    )
    logging.basicConfig(
        level=logging.INFO,
        handlers=[handler],
    )

    factory = driver_factory.DriverFactory(
        honeydew_config=generate_honeydew_config(),
        multi_device=args.multi_device,
        config_path=args.config_yaml_path,
        params_path=args.params_yaml_path,
        ssh_path=os.path.abspath(args.ssh_path) if args.ssh_path else None,
        target_address_type=args.target_address_type,
        ap_ip=args.ap_ip,
        ap_ssh_port=args.ap_ssh_port,
        ap_ssh_key=os.path.abspath(args.ap_ssh_key)
        if args.ap_ssh_key
        else None,
        ssh_key=os.path.abspath(args.ssh_key) if args.ssh_key else None,
    )
    driver = factory.get_driver()

    try:

        def maybe_timedelta(seconds: int | None) -> timedelta | None:
            if seconds is not None:
                return timedelta(seconds=seconds)
            return None

        # Use the same Python runtime for Mobly test execution as the one that's
        # currently running this Mobly driver script.
        mobly_driver.run(
            driver=driver,
            python_path=sys.executable,
            test_path=args.mobly_test_path,
            test_cases=args.test_cases,
            timeout=maybe_timedelta(args.test_timeout_sec),
            cleanup_period=maybe_timedelta(args.cleanup_period_sec),
            verbose=args.v,
            hermetic=args.hermetic,
            list_mobly_tests=args.list_mobly_tests,
        )
    except mobly_driver.MoblyTestFailureException as mtfe:
        # If an exception escapes a python process, the exit code is set to 1.
        # The goal here is to explicitly exit with the return code in mtfe, so
        # the exception must be caught...but that suppresses the traceback.
        # Print it explicitly to behave consistently with how other exceptions
        # coming out of Mobly appear in logs.
        # Note that `raise SystemExit(mtfe.return_code) from mtfe` doesn't
        # result in a traceback, because raising SystemExit is how python
        # programs exit gracefully.
        traceback.print_exc()
        sys.exit(mtfe.return_code)


def generate_honeydew_config() -> dict[str, Any]:
    """Generates and returns the Honeydew config.

    Generated Honeydew config will be in the following format:
    {
        "transports": {
            <transport_name>: {
                <key>: <value>,
                ...
            }
            ...
        }
        "affordances": {
            <affordance_name>: {
                <key>: <value>,
                ...
            }
            ...
        }
    }

    Example:
    {
        "transports": {
            "ffx": {
                "path": "/ffx/path",
                "subtools_search_path": "/subtools/path",
            }
        }
        "affordances": {
            "bluetooth": {
                "implementation": "fuchsia-controller"
            }
        }
    }
    """
    ffx_config: dict[str, Any] = {}
    ffx_config["path"] = os.path.abspath(args.ffx_path)
    if args.ffx_subtools_path:
        ffx_config["subtools_search_path"] = os.path.abspath(
            args.ffx_subtools_path
        )
    if args.ssh_auth_sock:
        ffx_config["ssh_auth_sock"] = os.path.abspath(args.ssh_auth_sock)
    if args.ssh_identities_only is not None:
        ffx_config["identities_only"] = args.ssh_identities_only

    honeydew_config: dict[str, Any] = {}
    if args.honeydew_config_json_path:
        with open(args.honeydew_config_json_path, "r") as honeydew_config_file:
            config = json.load(honeydew_config_file)
        honeydew_config.update(config)
    if "transports" not in honeydew_config:
        honeydew_config["transports"] = {}
    if "ffx" not in honeydew_config["transports"]:
        honeydew_config["transports"]["ffx"] = {}
    honeydew_config["transports"]["ffx"].update(ffx_config)

    return honeydew_config
