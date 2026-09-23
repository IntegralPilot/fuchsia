# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
# [START full_code]
import argparse
import asyncio

# [START required_import_block]
import fidl_fuchsia_buildinfo as f_buildinfo
import fidl_fuchsia_developer_remotecontrol as f_remote_control
from fuchsia_controller_py import Context

# [END required_import_block]

USAGE = "This script prints the result of RemoteControl.IdentifyHost on Fuchsia targets."


async def main() -> None:
    params = argparse.ArgumentParser(usage=USAGE)
    params.add_argument(
        "target_ips",
        help="Fuchsia target IP addresses",
        nargs="+",
    )
    args = params.parse_args()

    # [START describe_host_function]
    # Return information about a Fuchsia target.
    async def describe_host(target_ip: str) -> str:
        ctx = Context(
            target=target_ip,
        )
        remote_control_proxy = f_remote_control.RemoteControlClient(
            ctx.connect_remote_control_proxy()
        )
        identify_host_response = (
            # The `.response` in the next line is a bit confusing. The .unwrap() extracts the
            # RemoteControlIdentifyHostResponse from the response field of the returned
            # RemoteControlIdentifyHostResult, but RemoteControlIdentifyHostResponse also contains a
            # response field. There are two nested response fields.
            (await remote_control_proxy.identify_host())
            .unwrap()
            .response
        )

        build_info_proxy = f_buildinfo.ProviderClient(
            ctx.connect_device_proxy(
                "/core/build-info", f_buildinfo.ProviderMarker
            )
        )
        buildinfo_response = await build_info_proxy.get_build_info()
        return f"""
    --- {target_ip} ---
    nodename: {identify_host_response.nodename}
    product_config: {buildinfo_response.build_info.product_config}
    board_config: {buildinfo_response.build_info.board_config}
    version: {buildinfo_response.build_info.version}
"""

    # [END describe_host_function]

    # Asynchronously await information from each Fuchsia target.
    results = await asyncio.gather(*map(describe_host, args.target_ips))

    print("Target Info Received:")
    print("\n".join(results))


# [END full_code]
