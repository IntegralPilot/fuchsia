# Copyright 2023 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import asyncio
import json
import subprocess
import tempfile

import fidl_fuchsia_tracing as tracing
import fidl_fuchsia_tracing_controller as tracing_controller
import fuchsia_async_extension
from mobly import asserts, test_runner
from mobly_controller import fuchsia_device

from fidl import AsyncSocket

TRACE2JSON = "tracing_runtime_deps/trace2json"


class FuchsiaControllerTests(fuchsia_async_extension.AsyncBaseTestClass):
    async def setup_class(self) -> None:
        self.fuchsia_devices: list[
            fuchsia_device.FuchsiaDevice
        ] = await self.register_controller(fuchsia_device)
        self.device = self.fuchsia_devices[0]
        self.device.set_ctx(self)

    async def test_fuchsia_device_get_known_categories(self) -> None:
        """Verifies that kernel:vm is an existing category for tracing on the device."""
        if self.device.ctx is None:
            raise ValueError(f"Device: {self.device.target} has no context")
        ch = self.device.ctx.connect_device_proxy(
            "core/trace_manager", tracing_controller.ProvisionerMarker
        )
        controller = tracing_controller.ProvisionerClient(ch)
        try:
            response = (await controller.get_known_categories()).unwrap()
        except AssertionError as e:
            raise AssertionError("Error retrieving known categories") from e

        categories = response.categories
        found_kernel_category = False
        for category in categories:
            if category.name == "kernel:vm":
                found_kernel_category = True
                break
        asserts.assert_true(
            found_kernel_category,
            msg="Was not able to find 'kernel.vm' category in known output",
        )

    async def test_fuchsia_device_tracing_start_stop(self) -> None:
        """Does a simple start and stop of tracing on a device."""
        if self.device.ctx is None:
            raise ValueError(f"Device: {self.device.target} has no context")
        ch = self.device.ctx.connect_device_proxy(
            "core/trace_manager", tracing_controller.ProvisionerMarker
        )
        provisioner = tracing_controller.ProvisionerClient(ch)
        categories = [
            "blobfs",
            "gfx",
            "system_metrics",
        ]
        config = tracing_controller.TraceConfig(
            buffer_size_megabytes_hint=4,
            categories=categories,
            buffering_mode=tracing.BufferingMode.ONESHOT,
        )
        _client, server = self.device.ctx.socket_create()
        client = AsyncSocket(_client)

        client_end, server_end = self.device.channel_create()

        provisioner.initialize_tracing(
            controller=server_end.take(), config=config, output=server.take()
        )
        controller = tracing_controller.SessionClient(client_end)

        await controller.start_tracing()
        socket_task = asyncio.create_task(client.read_all())
        await asyncio.sleep(10)
        try:
            stop_tracing_response = (
                await controller.stop_tracing(write_results=True)
            ).unwrap()
        except AssertionError as e:
            raise AssertionError("Error stopping tracing") from e

        assert (
            stop_tracing_response.provider_stats is not None
        ), "Stop result provider stats should not be None."
        asserts.assert_true(
            len(stop_tracing_response.provider_stats) > 0,
            msg="Stop result provider stats should not be empty.",
        )

        # Closing the channel will terminate tracing
        controller.close_cleanly()

        raw_trace = await socket_task
        asserts.assert_equal(type(raw_trace), bytearray)
        asserts.assert_true(
            len(raw_trace) > 0, msg="Output bytes should not be empty."
        )

        with tempfile.NamedTemporaryFile(
            mode="wb", suffix=".fxt"
        ) as trace_fxt, tempfile.NamedTemporaryFile(
            mode="w+", suffix=".json", encoding="utf8"
        ) as trace_json:
            trace_fxt.write(raw_trace)
            trace_fxt.flush()

            subprocess.check_call(
                [
                    TRACE2JSON,
                    f"--input-file={trace_fxt.name}",
                    f"--output-file={trace_json.name}",
                ]
            )
            js_obj = json.load(trace_json)
        asserts.assert_true(
            js_obj.get("traceEvents") is not None,
            "Expected traceEvents to be present",
        )
        for trace_event in js_obj["traceEvents"]:
            trace_cat = trace_event["cat"]
            asserts.assert_true(
                trace_cat in categories,
                msg=f"Found unexpected category that isn't part of trace: {trace_cat}",
            )


if __name__ == "__main__":
    test_runner.main()
