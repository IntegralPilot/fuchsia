#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Unit tests for the perf metric publishing code."""

import dataclasses
import signal
import tempfile
import time
import unittest
import unittest.mock as mock
from pathlib import Path

from power import powerserver, sampler

_METRIC_NAME = "M3tr1cN4m3"
_MEASUREPOWER_PATH = "path/to/power"


class PowerSamplerTest(unittest.TestCase):
    """Tests for PowerSampler"""

    def setUp(self) -> None:
        self.output_dir = tempfile.TemporaryDirectory()
        self.output_dir_path = Path(self.output_dir.name)
        self.expected_csv_output_path = Path(
            self.output_dir_path, f"{_METRIC_NAME}_power_samples.csv"
        )
        self.assertFalse(self.expected_csv_output_path.exists())

        self.default_config = sampler.PowerSamplerConfig(
            output_dir=str(self.output_dir_path),
            metric_name=_METRIC_NAME,
            measurepower_path=None,
        )

        self.config_width_measurepower_path = dataclasses.replace(
            self.default_config, measurepower_path=_MEASUREPOWER_PATH
        )

    def test_sampler_without_measurepower(self) -> None:
        """Tests that PowerSampler creates zero results when not given a path to a measurepower binary."""
        with mock.patch("os.environ.get", return_value=None):
            power_sampler = powerserver.create_power_sampler(
                self.default_config
            )

        with mock.patch.object(time, "time", return_value=5):
            power_sampler.start()

        with mock.patch.object(time, "time", return_value=10):
            power_sampler.stop()

        self.assertFalse(power_sampler.has_samples())
        self.assertFalse(power_sampler.extract_samples())

    def test_sampler_with_measurepower(self) -> None:
        """Tests PowerSampler when given a path to a measurepower binary.

        The sampler should interact with the binary via subprocess.Popen
        and an intermediate csv file.
        """
        power_sampler = powerserver.create_power_sampler(
            self.config_width_measurepower_path, fallback_to_stub=False
        )

        with mock.patch("subprocess.Popen") as mock_popen:
            mock_proc = mock_popen.return_value
            mock_proc.poll.return_value = None

            # Fake output from mock_proc
            self.expected_csv_output_path.write_text(
                """Mandatory Timestamp,Current,Voltage
0,0,12
1000000000,25,12
2000000000,100,12
4000000000,75,12
""",
                encoding="utf-8",
            )

            power_sampler.start()

            mock_popen.assert_called()
            self.assertEqual(
                mock_popen.call_args.args[0],
                [
                    _MEASUREPOWER_PATH,
                    "-format",
                    "csv",
                    "-out",
                    str(self.expected_csv_output_path),
                ],
            )

            mock_proc.wait.return_value = 0
            power_sampler.stop()

            mock_proc.send_signal.assert_called_with(signal.SIGINT)
        self.assertEqual(
            power_sampler.extract_samples(),
            [
                sampler.Sample("0", "0", "12", None),
                sampler.Sample("1000000000", "25", "12", None),
                sampler.Sample("2000000000", "100", "12", None),
                sampler.Sample("4000000000", "75", "12", None),
            ],
        )

    @mock.patch("subprocess.Popen")
    @mock.patch("time.time")
    @mock.patch("time.sleep")
    def test_sampler_with_measurepower_timeout(
        self,
        mock_sleep: mock.MagicMock,
        mock_time: mock.MagicMock,
        mock_popen: mock.MagicMock,
    ) -> None:
        """Tests the sampler with a measurepower binary path that times out"""
        power_sampler = powerserver.create_power_sampler(
            self.config_width_measurepower_path, fallback_to_stub=False
        )

        mock_proc = mock_popen.return_value
        mock_proc.poll.return_value = None

        self.assertFalse(self.expected_csv_output_path.exists())

        # # Fake current time:
        mock_time.side_effect = [0, 30, 61]
        with self.assertRaises(TimeoutError):
            power_sampler.start()

        mock_sleep.assert_called_with(1)

    def test_create_power_sampler_without_measurepower(self) -> None:
        with mock.patch("os.environ.get", return_value=None):
            power_sampler = powerserver.create_power_sampler(
                self.default_config
            )
            self.assertIsInstance(power_sampler, powerserver._NoopPowerSampler)

    def test_create_power_sampler_without_measurepower_without_fallback_to_stub(
        self,
    ) -> None:
        with mock.patch("os.environ.get", return_value=None):
            with self.assertRaisesRegex(
                RuntimeError, ".* env variable must be set"
            ):
                powerserver.create_power_sampler(
                    self.default_config, fallback_to_stub=False
                )

    @mock.patch("power.powerserver.resources.path")
    @mock.patch("os.stat")
    @mock.patch("os.chmod")
    def test_with_hermetic_monsoon_power_sampler(
        self,
        mock_chmod: mock.MagicMock,
        mock_stat: mock.MagicMock,
        mock_path: mock.MagicMock,
    ) -> None:
        """Verifies the sampler correctly loads bundled resources."""
        # Mock the context manager returned by resources.path
        mock_path.return_value.__enter__.return_value = Path("/fake/path.pyz")
        with powerserver.with_hermetic_monsoon_power_sampler(
            str(self.output_dir_path), _METRIC_NAME, "12345"
        ) as s:
            assert isinstance(s, powerserver._RealPowerSampler)
            # Check that the serial number was passed through
            self.assertEqual(s._monsoon_serial, "12345")
            # Verify the path was correctly rebased/stringified
            self.assertEqual(s._config.measurepower_path, "/fake/path.pyz")
            # Verify the hermetic flag is set
            self.assertTrue(s._config.use_hermetic_measurepower)

    def test_create_power_sampler_with_measurepower_env_var(self) -> None:
        with mock.patch("os.environ.get", return_value="path/to/power"):
            power_sampler = powerserver.create_power_sampler(
                self.config_width_measurepower_path
            )
            self.assertIsInstance(power_sampler, powerserver._RealPowerSampler)

    def test_weighted_average(self) -> None:
        vals = [3, 2, 3, 4]
        weights = [1, 1, 1, 1]
        self.assertEqual(powerserver.weighted_average(vals, weights), 3)

        vals = [3, 2, 3, 4]
        weights = [1, 2, 3, 4]
        # (3 + 4 + 9 + 16) / 10 = 3.2
        self.assertEqual(powerserver.weighted_average(vals, weights), 3.2)

    def test_cross_correlate_arg_max(self) -> None:
        signal = [1, 2, 3, 4, 5, 6]
        feature = [1]
        self.assertEqual(
            powerserver.cross_correlate_arg_max(signal, feature), (6, 5)
        )

        signal = [1, 2, 3, 4, 5, 6]
        feature = [1, 2]
        self.assertEqual(
            powerserver.cross_correlate_arg_max(signal, feature), (17, 4)
        )

        signal = [0, 0, 0, 1, 4, 3, 2, 0]
        feature = [1, 4, 3, 2]
        self.assertEqual(
            powerserver.cross_correlate_arg_max(signal, feature), (30, 3)
        )

        large_signal = list(range(30000))
        large_feature = list(range(20000))
        self.assertEqual(
            powerserver.cross_correlate_arg_max(large_signal, large_feature),
            (4666366670000, 10000),
        )

    def test_normalize(self) -> None:
        signal = [0, 1, 2, 3, 4, 5]
        normalized = powerserver.normalize(signal)
        expected = [0, 0.2, 0.4, 0.6, 0.8, 1.0]
        self.assertEqual(normalized, expected)

    def test_normalized_xcorrelate(self) -> None:
        signal = [5, 0, 0, 0, 0, 2, 2, 2, 2, 2]
        feature = [10, 5, 5, 5, 5]

        # If we cross correlate without normalizing, we correlate best with the 2s
        self.assertEqual(
            powerserver.cross_correlate_arg_max(signal, feature), (60, 5)
        )

        # But if we normalize first
        # We'll have
        #
        # signal: [1, 0, 0, 0, 0, .4, .4, .4, .4]
        # feature: [1, 0, 0, 0, 0]
        #
        # which correctly correlates best with the beginning
        self.assertEqual(
            powerserver.cross_correlate_arg_max(
                powerserver.normalize(signal),
                powerserver.normalize(feature),
            ),
            (1.0, 0),
        )
