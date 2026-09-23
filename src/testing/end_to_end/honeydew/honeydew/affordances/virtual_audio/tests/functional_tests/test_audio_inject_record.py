# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Mobly test for Audio affordance."""

import logging
import os
import time
from datetime import timedelta

import fuchsia_base_test
from honeydew.affordances.virtual_audio.types import WaitForQuietResult
from mobly import test_runner

_AUDIO_FILE_INPUT = "audio_runtime_deps/sine_wave.wav"
_LOGGER: logging.Logger = logging.getLogger(__name__)


class AudioAffordanceTests(fuchsia_base_test.FuchsiaBaseTest):
    """Audio affordance tests"""

    async def test_audio(self) -> None:
        responseAudio = await self.dut.virtual_audio.capture()
        inputResponse = await self.dut.virtual_audio.inject(_AUDIO_FILE_INPUT)

        await inputResponse.wait_until_injection_is_done()

        time.sleep(5)

        data = await responseAudio.stop_and_extract_response()

        output_path = os.path.join(
            os.getenv("FUCHSIA_TEST_OUTDIR") or "", "response.wav"
        )
        _LOGGER.info("Got %d bytes", len(data))
        _LOGGER.info("Writing to %s", output_path)

        with open(
            output_path,
            "wb",
        ) as f:
            f.write(data)

    async def test_triggered_capture(self) -> None:
        quiet_result = await self.dut.virtual_audio.wait_for_quiet(
            requested_quiet_period=timedelta(seconds=2),
            optional_maximum_time_to_wait_for_quiet=timedelta(seconds=60),
        )
        assert quiet_result == WaitForQuietResult.SUCCESS

        await self.dut.virtual_audio.queue_triggered_capture(
            maximum_time_to_capture_audio=timedelta(seconds=5),
            maximum_time_to_wait_for_sound=timedelta(seconds=5),
            optional_quiet_time_before_stopping_capture=timedelta(seconds=1),
        )
        inputResponse = await self.dut.virtual_audio.inject(_AUDIO_FILE_INPUT)
        await inputResponse.wait_until_injection_is_done()

        capture_result = (
            await self.dut.virtual_audio.wait_for_triggered_capture()
        )
        data = b""
        if capture_result.audio_data is not None:
            _LOGGER.info("Retrieved audio data from the device")
            data = capture_result.audio_data
        else:
            _LOGGER.warning(
                "No audio data retrieved from the device. This may be OK depending on if this device is expected to play audio."
            )

        output_path = os.path.join(
            os.getenv("FUCHSIA_TEST_OUTDIR") or "", "response.wav"
        )
        _LOGGER.info("Got %d bytes", len(data))
        _LOGGER.info("Writing to %s", output_path)

        with open(
            output_path,
            "wb",
        ) as f:
            f.write(data)


if __name__ == "__main__":
    test_runner.main()
