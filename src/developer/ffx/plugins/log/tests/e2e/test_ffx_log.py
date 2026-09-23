# Copyright 2026 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Lacewing E2E test verifying logging functionality."""

import asyncio
import json
import logging
import re
import time
from typing import Any

import fuchsia_base_test
from honeydew.transports.ffx.types import MachineFormat
from honeydew.typing import custom_types
from mobly import asserts, test_runner

_LOGGER: logging.Logger = logging.getLogger(__name__)


class FfxLogVerificationTest(fuchsia_base_test.FuchsiaBaseTest):
    """Test class for verifying logging functionality."""

    async def setup_test(self) -> None:
        """Called automatically before each test case."""
        await super().setup_test()

    async def _collect_streaming_logs(
        self,
        cmd_args: list[str],
        machine_format: MachineFormat,
        test_msg_prefix: str,
        expected_count: int,
        timeout: float = 60.0,
    ) -> dict[str, dict[str, Any]]:
        """Starts a background ffx/ssh command, reads stdout line-by-line, and collects matching entries."""
        cmd = self.dut.ffx.generate_ffx_cmd(
            cmd_args,
            machine=machine_format,
        )
        process = await asyncio.create_subprocess_exec(
            *cmd,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL,
        )

        captured_entries: dict[str, dict[str, Any]] = {}
        log_pattern = re.compile(rf"{re.escape(test_msg_prefix)}-(\d+)(?!\d)")

        async def read_stdout() -> None:
            assert process.stdout is not None
            while True:
                line_bytes = await process.stdout.readline()
                if not line_bytes:
                    break
                line = line_bytes.decode("utf-8", errors="ignore").strip()
                if not line:
                    continue
                try:
                    log_line = json.loads(line)
                    target_log = log_line["data"]["TargetLog"]
                    msg = target_log["payload"]["root"]["message"]["value"]
                    match = log_pattern.search(msg)
                    if match:
                        idx = int(match.group(1))
                        if 0 <= idx < expected_count:
                            candidate = f"{test_msg_prefix}-{idx}"
                            if candidate not in captured_entries:
                                captured_entries[candidate] = target_log
                        if len(captured_entries) >= expected_count:
                            break
                except (json.JSONDecodeError, KeyError, TypeError):
                    pass

        try:
            await asyncio.wait_for(read_stdout(), timeout=timeout)
        except asyncio.TimeoutError:
            _LOGGER.warning(
                f"Timeout reached while streaming logs for {cmd_args}. Found {len(captured_entries)}/{expected_count} logs."
            )
        finally:
            try:
                process.kill()
                await asyncio.wait_for(process.wait(), timeout=5.0)
            except Exception:
                # This may be expected if the process couldn't be killed because
                # it already exited.
                pass

        return captured_entries

    async def test_logging_batching_and_formats(self) -> None:
        """Emits a batch of logs and validates log_listener and ffx log."""
        num_logs = 10
        timestamp_sec = int(time.time())
        test_msg_prefix = f"FfxLogE2EBatchLog-{timestamp_sec}"

        # 1. Start the streaming readers as background tasks BEFORE we emit any logs.
        fxt_task = asyncio.create_task(
            self._collect_streaming_logs(
                cmd_args=[
                    "target",
                    "ssh",
                    "log_listener --encoding fxt --json",
                ],
                machine_format=MachineFormat.RAW,
                test_msg_prefix=test_msg_prefix,
                expected_count=num_logs,
                timeout=60.0,
            )
        )
        json_task = asyncio.create_task(
            self._collect_streaming_logs(
                cmd_args=[
                    "target",
                    "ssh",
                    "log_listener --encoding json --json",
                ],
                machine_format=MachineFormat.RAW,
                test_msg_prefix=test_msg_prefix,
                expected_count=num_logs,
                timeout=60.0,
            )
        )
        ffx_task = asyncio.create_task(
            self._collect_streaming_logs(
                cmd_args=["log", "--symbolize", "off"],
                machine_format=MachineFormat.JSON,
                test_msg_prefix=test_msg_prefix,
                expected_count=num_logs,
                timeout=60.0,
            )
        )

        # 2. Emit a batch of unique logs to validate batching.
        for i in range(num_logs):
            msg = f"{test_msg_prefix}-{i}"
            await self.dut.log_message_to_device(
                message=msg,
                level=custom_types.LEVEL.INFO,
            )

        # 3. Await all streaming tasks to complete or timeout.
        fxt_entries, json_entries, ffx_entries = await asyncio.gather(
            fxt_task, json_task, ffx_task
        )

        # Verify that we correctly received the batch (at least some, but we expect all 10)
        asserts.assert_equal(
            len(fxt_entries),
            num_logs,
            f"Expected {num_logs} logs in FXT log_listener output, but found {len(fxt_entries)}",
        )
        asserts.assert_equal(
            len(json_entries),
            num_logs,
            f"Expected {num_logs} logs in JSON log_listener output, but found {len(json_entries)}",
        )
        asserts.assert_equal(
            len(ffx_entries),
            num_logs,
            f"Expected {num_logs} logs in FFX log output, but found {len(ffx_entries)}",
        )

        # 4. Ensure JSON, FXT, and FFX formatting is consistent and identical.
        for i in range(num_logs):
            msg = f"{test_msg_prefix}-{i}"
            fxt_entry = fxt_entries[msg]
            json_entry = json_entries[msg]
            ffx_entry = ffx_entries[msg]

            # Validate that the component moniker contains remote-control
            # (or log_message_to_device sender context).
            for name, entry in [
                ("FXT", fxt_entry),
                ("JSON", json_entry),
                ("FFX", ffx_entry),
            ]:
                asserts.assert_true(
                    any(
                        expected in entry.get("moniker", "")
                        for expected in ["remote-control", "remote"]
                    ),
                    f"Unexpected {name} moniker: {entry.get('moniker')}",
                )

            # Compare structure and key fields to ensure identical formatting between all outputs
            for field in ["moniker", "version"]:
                asserts.assert_equal(
                    fxt_entry.get(field),
                    json_entry.get(field),
                    f"Mismatch in '{field}' between FXT and JSON for log: {msg}",
                )
                asserts.assert_equal(
                    json_entry.get(field),
                    ffx_entry.get(field),
                    f"Mismatch in '{field}' between JSON and FFX for log: {msg}",
                )

            # Compare metadata fields
            fxt_meta = fxt_entry.get("metadata", {})
            json_meta = json_entry.get("metadata", {})
            ffx_meta = ffx_entry.get("metadata", {})
            for field in ["severity", "component_url"]:
                asserts.assert_equal(
                    fxt_meta.get(field),
                    json_meta.get(field),
                    f"Mismatch in metadata '{field}' between FXT and JSON for log: {msg}",
                )
                asserts.assert_equal(
                    json_meta.get(field),
                    ffx_meta.get(field),
                    f"Mismatch in metadata '{field}' between JSON and FFX for log: {msg}",
                )


if __name__ == "__main__":
    test_runner.main()
