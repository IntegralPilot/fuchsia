# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import pathlib
import subprocess
import unittest

# NB: These must be kept in sync with the values in BUILD.gn.
component_name = "bar"
package_name = "for-test"
expected_value_in_policy = "check this string!"


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Check that a 'bad' config is rejected."
    )
    parser.add_argument(
        "--ffx-bin",
        type=pathlib.Path,
        required=True,
        help="Path to the ffx binary.",
    )
    parser.add_argument(
        "--policy",
        type=pathlib.Path,
        required=True,
        help="Path to JSON5 policy file which should produce errors.",
    )
    parser.add_argument(
        "--depfile",
        type=pathlib.Path,
        required=True,
        help="Path to ninja depfile to write.",
    )
    parser.add_argument(
        "--product-bundle",
        type=pathlib.Path,
        required=True,
        help="Path to the product bundle.",
    )
    parser.add_argument(
        "--component-tree-config",
        type=pathlib.Path,
        required=False,
        help="Path to the component tree config.",
    )
    args = parser.parse_args()

    # Assume we're in the root build dir right now and that is where we'll find ffx env.
    ffx_env_path = "./.ffx.env"

    # Imitate the configuration in //src/developer/ffx/build/ffx_action.gni.
    base_config = [
        "ffx.analytics.disabled=true",
        "daemon.autostart=false",
        "log.enabled=false",
    ]

    ffx_args = [args.ffx_bin]
    for c in base_config:
        ffx_args += ["--config", c]
    ffx_args += [
        "--env",
        ffx_env_path,
        "scrutiny",
        "verify",
        "structured-config",
        "--policy",
        args.policy,
        "--product-bundle",
        args.product_bundle,
    ]

    if args.component_tree_config:
        ffx_args += [
            "--component-tree-config",
            args.component_tree_config,
        ]

    test = unittest.TestCase()

    output = subprocess.run(ffx_args, capture_output=True)
    test.assertNotEqual(
        output.returncode, 0, "ffx scrutiny verify must have failed"
    )

    stderr = output.stderr.decode("UTF-8")
    expected_error = f"""
└── fuchsia-pkg://fuchsia.com/{package_name}#meta/{component_name}.cm
      └── `asserted_by_scrutiny_test` has a different value ("{expected_value_in_policy}") than expected ("not the string that was packaged").
      └── `mutable_by_parent_config` has an expected value in the policy which could be overridden at runtime by ConfigMutability(PARENT)."""
    test.assertIn(
        expected_error, stderr, "error message must contain expected failures"
    )
