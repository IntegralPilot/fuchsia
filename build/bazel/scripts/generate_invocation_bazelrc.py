#!/usr/bin/env python3
# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Create a bazelrc file that contains unique values per invocation
based on environment variables such as uuids and paths to temporary sockets.

The generated bazelrc file is closely related to
template.remote_services.bazelrc in that it amends existing configuration
definitions.

This script is intended to work for both developer (fx) and infra builds.
"""

import argparse
import os
import sys
from collections.abc import Iterable

_SCRIPT_DIR = os.path.dirname(__file__)


def bbid_link(bbid: str) -> str:
    # Builds launched directly by the 'led' tool look different
    # from other infra builds.
    if "/led/" in bbid:
        return f"http://go/lucibuild/{bbid}/+/build.proto"
    else:
        return f"http://go/bbid/{bbid}"


def metadata_option(key: str, value: str) -> str:
    """Returns the equivalent bazel option for passing metadata."""
    return f"--build_metadata={key}={value}"


def build_config_option(config: str, options: str) -> str:
    """Returns a bazelrc command that maps options into a config definition."""
    return f"build:{config} {options}"


def metadata_bazelrc(env: dict[str, str]) -> Iterable[str]:
    uuid = env.get("FX_BUILD_UUID")  # set by 'fx build'
    bbid = env.get("BUILDBUCKET_ID")  # set by infra builds
    if not uuid and not bbid:
        # This is an out-of-build invocation that we don't care to record.
        return

    if uuid:
        # Link all invocations directly to the top-level FX_BUILD_UUID.
        yield build_config_option(
            "_bes_common",
            metadata_option("FX_BUILD_UUID", uuid),
        )

    if bbid:
        # Link all invocations directly to the top-level buildbucket.
        yield build_config_option(
            "_bes_common",
            metadata_option("BUILDBUCKET_ID", bbid),
        )

    # LINT.IfChange(related_invocations_env_vars)
    parent_build_id = env.get("RESULTSTORE_PARENT_BUILD_ID")
    if not parent_build_id:
        # This is a top-level build invocation.
        # For infra builds, reference the bbid (buildbucket).
        if bbid:
            yield build_config_option(
                "_bes_common",
                metadata_option("PARENT_BUILD_ID", bbid),
            )
            yield build_config_option(
                "_bes_common",
                metadata_option("PARENT_BUILD_LINK", bbid_link(bbid)),
            )

        # For fx-builds, leave PARENT_BUILD_ID, PARENT_BUILD_LINK,
        # and SIBLING_BUILDS_LINK unset.

    else:
        # This is a sub-build.
        # Take the values provided by the environment from a parent build.
        yield build_config_option(
            "_bes_common",
            metadata_option("PARENT_BUILD_ID", parent_build_id),
        )
        # URLs are already based on whether sponge/resultstore is used,
        # so there is no need to select here.
        parent_link = env.get("RESULTSTORE_PARENT_BUILD_LINK")
        if parent_link:
            yield build_config_option(
                "_bes_common",
                metadata_option("PARENT_BUILD_LINK", parent_link),
            )

        siblings_link = env.get("RESULTSTORE_SIBLING_BUILDS_LINK")
        if siblings_link:
            yield build_config_option(
                "_bes_common",
                metadata_option("SIBLING_BUILDS_LINK", siblings_link),
            )

    # LINT.ThenChange(
    #   //build/bazel/wrapper.bazel.sh:related_invocations_env_vars,
    #   //build/bazel/scripts/rsninja.sh:related_invocations_env_vars
    # )


def service_proxies_bazelrc(env: dict[str, str]) -> Iterable[str]:
    # LINT.IfChange(bazel_socket_env_vars)
    # Redirect traffic to proxies (infra only).
    # Remote service configuration.
    # TODO(https://fxbug.dev/450234102): Deprecate legacy BAZEL_rbe_socket_path after recipes migrate.
    rbe_socket_path = env.get("FX_INTERNAL_BAZEL_RBE_SOCKET_PATH") or env.get(
        "BAZEL_rbe_socket_path"
    )
    if rbe_socket_path:
        yield build_config_option(
            "_remote_common", f"--remote_proxy=unix://{rbe_socket_path}"
        )
    # sponge and resultstore are mutually exclusive, so only one of the
    # following two --bes_proxy options will ever apply.
    sponge_socket_path = env.get("BAZEL_sponge_socket_path")
    if sponge_socket_path:
        yield build_config_option(
            "sponge_infra", f"--bes_proxy=unix://{sponge_socket_path}"
        )

    # TODO(https://fxbug.dev/481021388): Deprecate legacy BAZEL_resultstore_socket_path after recipes migrate.
    resultstore_socket_path = env.get(
        "FX_INTERNAL_BAZEL_RESULTSTORE_SOCKET_PATH"
    ) or env.get("BAZEL_resultstore_socket_path")
    if resultstore_socket_path:
        yield build_config_option(
            "resultstore_infra", f"--bes_proxy=unix://{resultstore_socket_path}"
        )
    # LINT.ThenChange(//build/scripts/main_build.py:bazel_socket_env_vars)


def generate_bazelrc(
    sub_builds_link: str, env: dict[str, str]
) -> Iterable[str]:
    header = [
        "# This bazelrc file contains ephemeral options that are not intended",
        "# to persist across build invocations.",
        "# AUTO-GENERATED - DO NOT EDIT!",
    ]
    yield from header

    if sub_builds_link:
        yield build_config_option(
            "_bes_common",
            metadata_option("SUB_BUILDS_LINK", sub_builds_link),
        )

    yield from metadata_bazelrc(env)

    yield from service_proxies_bazelrc(env)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sub_builds_link",
        type=str,
        default="",
        help="The URL to a search query that finds sub-builds of this invocation.",
    )
    args = parser.parse_args(argv)
    for line in generate_bazelrc(args.sub_builds_link, os.environ):
        print(line)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
