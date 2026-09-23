#!/usr/bin/env fuchsia-vendored-python
# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Build a target for a particular target_cpu and api_level."""

import argparse
import json
import logging
import multiprocessing
import os
import shlex
import shutil
import subprocess
import sys
import time
from pathlib import Path

logger = logging.getLogger("subbuild.py")

_ARGS_GN_TEMPLATE = r"""# Auto-generated - DO NOT EDIT
target_cpu = "{cpu}"
build_info_board = "{cpu}"
build_info_product = "bringup"
compilation_mode = "{compilation_mode}"

cxx_rbe_enable = {cxx_rbe_enable}
link_rbe_enable = {link_rbe_enable}
rust_rbe_enable = {rust_rbe_enable}
build_only_labels = [{sdk_labels_list}]
"""


def get_host_platform() -> str:
    """Return host platform name, following Fuchsia conventions."""
    if sys.platform == "linux":
        return "linux"
    elif sys.platform == "darwin":
        return "mac"
    else:
        return os.uname().sysname


def get_host_arch() -> str:
    """Return host CPU architecture, following Fuchsia conventions."""
    host_arch = os.uname().machine
    if host_arch == "x86_64":
        return "x64"
    elif host_arch.startswith(("armv8", "aarch64")):
        return "arm64"
    else:
        return host_arch


def get_host_tag() -> str:
    """Return host tag, following Fuchsia conventions."""
    return "%s-%s" % (get_host_platform(), get_host_arch())


def write_file_if_changed(path: Path, content: str) -> bool:
    """Write |content| into |path| if needed. Return True on write."""
    if path.exists() and path.read_text() == content:
        # Nothing to do
        return False

    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    return True


def command_args_to_string(
    args: list[str], env: dict[str, str] | None, cwd: Path | str | None
) -> str:
    elements = []
    if cwd:
        elements.append(f"cd {shlex.quote(str(cwd))} &&")
    if env:
        for key, value in env.items():
            if key not in os.environ or value != os.environ[key]:
                elements += [
                    "%s=%s" % (name, shlex.quote(value))
                    for name, value in sorted(env.items())
                ]
    elements += [shlex.quote(str(a)) for a in args]
    return " ".join(elements)


def run_command(
    args: list[str],
    capture_output: bool = False,
    env: dict[str, str] | None = None,
    cwd: Path | str | None = None,
) -> subprocess.CompletedProcess:
    """Run a command.

    Args:
        args: A list of strings or Path items (each one of them will be
            converted to a string for convenience).
        **kwargs: other arguments passed to subprocess
    Returns:
        a subprocess.run() result value.
    """
    logger.info("RUN: " + command_args_to_string(args, env, cwd))
    start_time = time.time()
    result = subprocess.run(
        [str(a) for a in args],
        env=env,
        cwd=cwd,
        capture_output=capture_output,
        text=True,
    )
    end_time = time.time()
    logger.info("DURATION: %.1fs" % (end_time - start_time))
    return result


def run_checked_command(
    args: list[str],
    capture_output: bool,
    env: dict[str, str] | None = None,
    cwd: Path | str | None = None,
) -> bool:
    """Run a command, return True if succeeds, False otherwise.

    Args:
        args: A list of strings or Path items (each one of them will be
            converted to a string for convenience).
        **kwargs: other arguments passed to subrpcoes
    Returns:
        True on success. In case of failure, print the command line and
        stdout + stderr, then return False.
    """
    try:
        ret = run_command(args, env=env, cwd=cwd, capture_output=capture_output)
        if ret.returncode == 0:
            return True
    except KeyboardInterrupt:
        # If the user interrupts a long-running command, do not print anything.
        return False

    args_str = command_args_to_string(args, env, cwd)
    logger.error(
        f"Exit code {ret.returncode} when running command: `{args_str}`\nOutput:\n{ret.stdout}\nError:\n{ret.stderr}\n"
    )
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--target-label",
        required=True,
        help="GN target to build.",
    )
    parser.add_argument(
        "--prebuilt-host-tools-dir",
        help="Build directory containing host tools.",
    )
    parser.add_argument(
        "--target-cpu",
        required=True,
        help="Target CPU for which to build the target.",
    )
    parser.add_argument(
        "--api-level",
        type=str,
        required=True,
        help="API level at which to build the target.",
    )
    parser.add_argument("--stamp-file", help="Optional output stamp file.")
    parser.add_argument(
        "--fuchsia-dir", required=True, help="Fuchsia source directory."
    )
    parser.add_argument(
        "--output-build-dir",
        required=True,
        help="Build dir for the subbuild",
    )
    parser.add_argument(
        "--upload-build-events",
        type=str,
        choices=["no", "resultstore", "resultstore_infra"],  # No Sponge yet
        default="no",
        help="Upload results metadata to the ResultStore service",
    )
    parser.add_argument(
        "--parallelism",
        type=int,
        default=multiprocessing.cpu_count(),
        help="Parallelism argument (-j) to pass to ninja.",
    )
    parser.add_argument(
        "--max-load-average",
        type=int,
        default=multiprocessing.cpu_count(),
        help="Max load average argument (-l) to pass to ninja.",
    )
    parser.add_argument(
        "--host-tag", help="Fuchsia host os/cpu tag used to find prebuilts."
    )
    parser.add_argument(
        "--use-jobserver",
        action="store_true",
        help="Use a jobserver pool setup by the parent Ninja process.",
    )
    parser.add_argument(
        "--verbose", action="store_true", help="Print more information."
    )
    parser.add_argument(
        "--clean", action="store_true", help="Force clean build."
    )
    parser.add_argument(
        "--setup-only",
        action="store_true",
        help="Only setup the output directory. Do not build.",
    )
    parser.add_argument(
        "--api-level-path",
        type=str,
        default=None,
        help="If set, write the API level to this file in the output directory.",
    )

    args = parser.parse_args()

    logging.basicConfig(
        stream=sys.stderr, level=logging.INFO if args.verbose else logging.WARN
    )

    fuchsia_dir = Path(args.fuchsia_dir)

    # Locate GN and Ninja prebuilts.
    if args.host_tag:
        host_tag = args.host_tag
    else:
        host_tag = get_host_tag()

    gn_path = fuchsia_dir / "prebuilt" / "third_party" / "gn" / host_tag / "gn"
    if not gn_path.exists():
        logger.error(f"Missing gn prebuilt binary: {gn_path}")
        return 1

    # Use a drop-in replacement for ninja that enables ResultStore passively.
    # Enabling ResultStore is triggered by the inherited environment variable
    # FX_INTERNAL_RESULTSTORE_NINJA, which is set by build/scripts/main_build.py.
    # When disabled, the un-needed wrappers will be bypassed down to raw ninja
    # with zero overhead.
    ninja_path = fuchsia_dir / "build" / "resultstore" / "rs-sub-ninja.sh"
    if not ninja_path.exists():
        logger.error(f"Missing ninja prebuilt binary: {ninja_path}")
        return 1

    ninja_cmd_prefix = [ninja_path]
    if not args.verbose:
        ninja_cmd_prefix.append("--quiet")

    def label_partition(target_label: str) -> tuple[str, str]:
        """Split an GN label into a (dir, name) pair."""
        # Expected format is //<dir>:<name>
        path, colon, name = target_label.partition(":")
        assert colon == ":" and path.startswith(
            "//"
        ), f"Invalid target label: {target_label}"
        return (path[2:], name)

    def label_to_ninja_target(target_label: str) -> str:
        """Convert GN label to Ninja target path."""
        target_dir, target_name = label_partition(target_label)
        return f"{target_dir}:{target_name}"

    target_cpu = args.target_cpu
    api_level = args.api_level

    build_dir = Path(args.output_build_dir)
    build_dir.mkdir(exist_ok=True, parents=True)
    logger.info(
        f"{build_dir}: Preparing sub-build, directory: {build_dir.resolve()}"
    )

    if args.clean and build_dir.exists():
        logger.info(f"{build_dir}: Cleaning build directory")
        run_command([*ninja_cmd_prefix, "-C", str(build_dir), "-t", "clean"])

    # Copy regenerator.py-created files from the root build dir to the subbuild dir.
    files = ["icu_build_config.json"]
    for file in files:
        source_file = build_dir.parent / file
        dest_file = build_dir / file
        shutil.copy2(source_file, dest_file)

    # LINT.IfChange(subbuild_config)
    subbuild_config_json_path = build_dir.parent / "subbuild_config.json"
    if not subbuild_config_json_path.exists():
        logger.error(f"Missing {subbuild_config_json_path}")
        return 1
    with subbuild_config_json_path.open("r") as f:
        subbuild_config = json.load(f)

    compilation_mode = subbuild_config["compilation_mode"]
    cxx_rbe_enable = subbuild_config["cxx_rbe_enable"]
    link_rbe_enable = subbuild_config["link_rbe_enable"]
    rust_rbe_enable = subbuild_config["rust_rbe_enable"]

    cxx_exec_strategy = subbuild_config.get("cxx_rbe_exec_strategy")
    link_exec_strategy = subbuild_config.get("link_rbe_exec_strategy")
    rust_exec_strategy = subbuild_config.get("rust_rbe_exec_strategy")

    sdk_id = subbuild_config.get("sdk_id")
    compress_debuginfo = subbuild_config.get("compress_debuginfo")
    update_goldens = subbuild_config.get("update_goldens", False)
    trace_build_actions = subbuild_config.get(
        "build_should_trace_actions", False
    )
    # LINT.ThenChange(//sdk/BUILD.gn:subbuild_config)

    args_gn_content = _ARGS_GN_TEMPLATE.format(
        cpu=target_cpu,
        compilation_mode=compilation_mode,
        cxx_rbe_enable="true" if cxx_rbe_enable else "false",
        link_rbe_enable="true" if link_rbe_enable else "false",
        rust_rbe_enable="true" if rust_rbe_enable else "false",
        sdk_labels_list=f'"{args.target_label}"',
    )

    if cxx_exec_strategy:
        args_gn_content += f'cxx_rbe_exec_strategy = "{cxx_exec_strategy}"\n'
    if link_exec_strategy:
        args_gn_content += f'link_rbe_exec_strategy = "{link_exec_strategy}"\n'
    if rust_exec_strategy:
        args_gn_content += f'rust_rbe_exec_strategy = "{rust_exec_strategy}"\n'

    if sdk_id:
        args_gn_content += f'sdk_id = "{sdk_id}"\n'

    if compress_debuginfo:
        args_gn_content += f'compress_debuginfo = "{compress_debuginfo}"\n'

    if args.prebuilt_host_tools_dir:
        # Reuse host tools from the top-level build. This assumes that
        # sub-builds cannot use host tools that were not already built by
        # the top-level build, as there is no way to inject dependencies
        # between the two build graphs.
        args_gn_content += f'host_tools_base_path_override = "{args.prebuilt_host_tools_dir}"\n'

    args_gn_content += "sdk_inside_sub_build = true\n"

    # Special API levels are passed to GN as a string; numeric API levels are
    # passed as ints.
    if api_level in ("NEXT", "HEAD", "PLATFORM"):
        gn_api_level = f'"{api_level}"'
    else:
        gn_api_level = str(int(api_level))

    args_gn_content += f"current_build_target_api_level = {gn_api_level}\n"

    if update_goldens:
        args_gn_content += "update_goldens = true\n"

    if trace_build_actions:
        args_gn_content += "build_should_trace_actions = true\n"

    logger.info(f"{build_dir}: args.gn content:\n{args_gn_content}")
    if (
        write_file_if_changed(build_dir / "args.gn", args_gn_content)
        or not (build_dir / "build.ninja").exists()
    ):
        if not run_checked_command(
            [
                gn_path,
                "--root=%s" % fuchsia_dir.resolve(),
                "--root-pattern=//:build_only",
                "gen",
                str(build_dir),
            ],
            capture_output=not args.verbose,
        ):
            return 1

    if not args.setup_only:
        # Adjust the NINJA_STATUS environment variable before launching Ninja
        # in order to add a prefix distinguishing its build actions from
        # the top-level ones.
        ninja_status = os.environ.get("NINJA_STATUS", "[%f/%t][%p/%w](%r) ")

        status_prefix = f"IDK_SUBBUILD_api_{api_level}_{target_cpu} "
        ninja_status = status_prefix + ninja_status
        ninja_env = {"NINJA_STATUS": ninja_status}

        ninja_edge_weights_path = os.path.relpath(
            "ninja_edge_weights.csv", build_dir
        )

        ninja_cmd = [
            *ninja_cmd_prefix,
            "--chrome_trace",
            "ninja_build_trace.json.gz",
            "--action_metrics_output",
            "ninja_action_metrics.json",
            f"--edge_weights_list={ninja_edge_weights_path}",
            "-C",
            str(build_dir),
        ]
        use_jobserver = bool(args.use_jobserver)
        if use_jobserver:
            # Check that the jobserver pool is setup by scanning the MAKEFLAGS
            # environment variable. If it does not contain --jobserver-auth
            # something is wrong, and the argument will be ignored.
            makeflags = os.environ.get("MAKEFLAGS", "")
            if "--jobserver-auth" not in makeflags:
                print(
                    f"WARNING: --use-jobserver used, but no jobserver pool in MAKEFLAGS [{makeflags}]",
                    file=sys.stderr,
                )
                use_jobserver = False

            # IMPORTANT NOTE: benchmarking shows that enforcing load-average limitation
            # with --jobserver drastically reduces overall performance when using
            # remote builders. Why exactly is still undetermined but for reference,
            # a minimal.x64 build configuration, on a 128 core workstation with RBE
            # enabled can clean-build //sdk:final_fuchsia_idk.exported in:
            #
            # - regular build:                            12m52.786s
            # - jobserver build (current implementation): 9m58.563s
            # - jobserver build (with -l<load_average>):  24m25.327s  (!?)
            #

        if not use_jobserver:
            # Only pass a -j value, not a loadaverage limit value, as Ninja appears to
            # incorrectly use the loadavg specified by the -l option, and may only
            # schedule one action at time for minutes on end.
            ninja_cmd += [
                "-j",
                args.parallelism,
            ]

        ninja_cmd += [
            label_to_ninja_target(args.target_label),
        ]

        if not run_checked_command(
            ninja_cmd,
            capture_output=not args.verbose,
            env=os.environ | ninja_env,
        ):
            return 1

        if args.api_level_path:
            api_level_path = Path(args.api_level_path)
            api_level_path.write_text(str(api_level))

    # Write stamp file if needed.
    if args.stamp_file:
        with open(args.stamp_file, "w") as f:
            f.write("")

    return 0


if __name__ == "__main__":
    sys.exit(main())
