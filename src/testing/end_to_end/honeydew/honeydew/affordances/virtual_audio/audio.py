# Copyright 2025 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Abstract base class for Audio affordance."""

import abc
from datetime import timedelta

from honeydew.affordances.virtual_audio import types


class VirtualAudio(abc.ABC):
    """Abstract base class for an async Audio affordance."""

    @abc.abstractmethod
    async def inject(self, wav_file: str) -> types.AudioInputWaiter:
        """Inject wav_file audio query.
        Args:
            wav_file: Audio .wav file

        Return:
            AudioInputWaiter: object. This object is used to wait until the input injection is done.

        Raises:
            VirtualAudioError: On failure
        """

    @abc.abstractmethod
    async def capture(self) -> types.AudioResponse:
        """Start to capture the audio response.

        Return:
            AudioResponse: object. This object is used to stop and extract the captured audio.

        Raises:
            VirtualAudioError: On failure
        """

    @abc.abstractmethod
    async def wait_for_quiet(
        self,
        requested_quiet_period: timedelta,
        optional_maximum_time_to_wait_for_quiet: timedelta | None = None,
    ) -> types.WaitForQuietResult:
        """Wait until there is no audio playing on the output audio device.

        Args:
            requested_quiet_period (timedelta): Return only after the
                output is quiet for this duration.
            maximum_time_to_wait_for_quiet (optional timedelta): Maximum
                duration to wait.

        Returns:
            WaitForQuietResult: object. Enumeration that reports the
                success of the operation.

        Raises:
            VirtualAudioError: On internal failure. See component logs
                for details.
        """

    @abc.abstractmethod
    async def queue_triggered_capture(
        self,
        maximum_time_to_wait_for_sound: timedelta,
        maximum_time_to_capture_audio: timedelta,
        optional_quiet_time_before_stopping_capture: timedelta | None,
    ) -> None:
        """Queue a triggered audio capture that starts when audio is detected
        and ends after a configurable period.

        This function returns when the capture is queued. Use
        `wait_for_triggered_capture` to wait for the result of this
        capture.

        Args:
            maximum_time_to_wait_for_sound (timedelta): Limit on how
                long to wait before recording should start.
            maximum_time_to_capture_audio (timedelta): Maximum duration
                of the recording.
            optional_quiet_time_before_stopping_capture (timedelta):
                If set, stop the capture early if this duration of quiet
                time is observed after recording started. If not set,
                continue capturing until `maximum_time_to_capture_audio`
                duration of audio is recorded.

        Raises:
            VirtualAudioError: On internal failure. See component logs
                for details.
        """

    @abc.abstractmethod
    async def wait_for_triggered_capture(self) -> types.TriggeredCaptureResult:
        """Wait for a previously queued triggered capture to complete.

        Returns:
            TriggeredCaptureResult: object. The result of the capture
                operation, containing information on whether recording was
                triggered and, if so, the recorded data.

        Raises:
            VirtualAudioError: On internal failure. See component logs
                for details.
        """
