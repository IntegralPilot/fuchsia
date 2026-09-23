# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import subprocess


def run_product_assembly(
    ffx_bin,
    platform,
    product,
    board_config,
    platform_artifacts,
    legacy_bundle,
    outdir,
    gendir,
    suppress_overrides_warning=False,
    extra_config=[],
    capture_output=False,
    **kwargs,
):
    """
    Run `ffx assembly product ...` with appropriate configuration and arguments for host tests.

    Useful if you need to test assembly and assert that it will fail, or pass custom configuration
    to your invocation of the tool.

    Assumes that the script calling this function is an action or host test invoked such that
    cwd=root_build_dir.

    Optional arguments can be passed by name.
    """

    # assume we're in the root build dir right now and that is where we'll find ffx env
    ffx_env_path = "./.ffx.env"

    base_config = [
        "assembly_enabled=true",
        # imitate the configuration in //src/developer/ffx/build/ffx_action.gni
        "ffx.analytics.disabled=true",
        "daemon.autostart=false",
        "log.enabled=false",
    ]

    args = [ffx_bin]
    for c in base_config + extra_config:
        args += ["--config", c]
    args += [
        "--env",
        ffx_env_path,
        "assembly",
        "product",
        "--product",
        product,
        "--board-config",
        board_config,
        "--platform-artifacts",
        platform_artifacts,
        "--outdir",
        outdir,
        "--gendir",
        gendir,
    ]
    if legacy_bundle:
        args += [
            "--legacy-bundle",
            legacy_bundle,
        ]

    if suppress_overrides_warning:
        args += ["--suppress-overrides-warning"]

    for arg_name, value in kwargs.items():
        args.append("--" + arg_name.replace("_", "-"))
        args.append(value)

    return subprocess.run(args, capture_output)
