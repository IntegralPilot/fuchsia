# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Test for profile.py"""

import unittest
from unittest.mock import Mock

from memory import profile
from reporting import metrics

MM2_OUTPUT = """
    {
        "ComponentDigest": {
            "kernel": {
                "memory_statistics": {
                    "total_bytes": 8588746752,
                    "free_bytes": 5056327680
                },
                "compression_statistics": {
                    "uncompressed_storage_bytes": 0
                }
            },
            "principals": [
                {
                    "id": 5,
                    "name": "bootstrap/fshost/fxfs",
                    "principal_type": "R",
                    "committed_private": 32927744,
                    "committed_scaled": 450043948.85779566,
                    "committed_total": 1436753920,
                    "populated_private": 32927744,
                    "populated_scaled": 450043948.85779566,
                    "populated_total": 1436753920,
                    "attributor": "root",
                    "processes": [
                        "fxfs.cm (13934)"
                    ],
                    "vmos": {
                        "[blobs]": {
                            "count": 1827,
                            "committed_private": 0,
                            "committed_scaled": 411303842.98151606,
                            "committed_total": 1385910272,
                            "populated_private": 0,
                            "populated_scaled": 411303842.98151606,
                            "populated_total": 1385910272
                        }
                    }
                }
            ],
            "undigested": 0,
            "digest": {
                "buckets": [
                    {
                        "name": "ZBI Buffer",
                        "committed_size": 0,
                        "populated_size": 0
                    },
                    {
                        "name": "[Addl]Graphics",
                        "populated_size": 65432,
                        "committed_size": 49152
                    }
                ]
            }
        }
    }
"""


class ProfileTest(unittest.TestCase):
    def test_capture_and_export_digest(self) -> None:
        def ffx_run_fake_implementation(args: list[str]) -> str:
            return MM2_OUTPUT

        dut = Mock()
        dut.ffx.run.side_effect = ffx_run_fake_implementation
        report = profile.capture(
            dut, buckets_metrics=".*Graphics", board_memory=8 * 1024**3
        )

        self.assertEqual(
            report.structured,
            [
                metrics.TestCaseResult(
                    label="Memory/Bucket/AddlGraphics/CommittedBytes",
                    unit=metrics.Unit.bytes,
                    values=(49152,),
                    doc="Total committed bytes in the bucket: [Addl]Graphics",
                ),
                metrics.TestCaseResult(
                    label="Memory/Bucket/AddlGraphics/PopulatedBytes",
                    unit=metrics.Unit.bytes,
                    values=(65432,),
                    doc="Total populated bytes in the bucket: [Addl]Graphics",
                ),
                metrics.TestCaseResult(
                    label="Memory/System/FuchsiaOSPopulatedBytes",
                    unit=metrics.Unit.bytes,
                    values=(1187840,),
                    doc="Fuchsia OS Populated Memory",
                ),
            ],
        )

    def test_capture_and_compute_metrics(self) -> None:
        def ffx_run_fake_implementation(args: list[str]) -> str:
            backend = args[args.index("--backend") + 1]
            if backend == "memory_monitor_1":
                raise RuntimeError("Boom")
            return MM2_OUTPUT

        dut = Mock()
        dut.ffx.run.side_effect = ffx_run_fake_implementation
        report = profile.capture(
            dut, principal_groups={"fxfs": "*/fxfs"}, board_memory=8 * 1024**3
        )

        self.assertEqual(
            report.structured,
            [
                metrics.TestCaseResult(
                    label="Memory/Principal/fxfs/PrivatePopulated",
                    unit=metrics.Unit.bytes,
                    values=(32927744,),
                    doc="Total populated bytes for private uncompressed memory "
                    "VMOs: fxfs",
                ),
                metrics.TestCaseResult(
                    label="Memory/System/FuchsiaOSPopulatedBytes",
                    unit=metrics.Unit.bytes,
                    values=(1187840,),
                    doc="Fuchsia OS Populated Memory",
                ),
            ],
        )
        self.maxDiff = None
        self.assertEqual(
            report.freeform,
            {
                "memory_profile": {
                    "kernel": {
                        "memory_statistics": {
                            "total_bytes": 8588746752,
                            "free_bytes": 5056327680,
                        },
                        "compression_statistics": {
                            "uncompressed_storage_bytes": 0,
                        },
                    },
                    "principals": [
                        {
                            "id": 5,
                            "name": "bootstrap/fshost/fxfs",
                            "principal_type": "R",
                            "committed_private": 32927744,
                            "committed_scaled": 450043948.85779566,
                            "committed_total": 1436753920,
                            "populated_private": 32927744,
                            "populated_scaled": 450043948.85779566,
                            "populated_total": 1436753920,
                            "attributor": "root",
                            "processes": ["fxfs.cm (13934)"],
                            "vmos": [
                                {
                                    "name": "[blobs]",
                                    "count": 1827,
                                    "committed_private": 0,
                                    "committed_scaled": 411303842.98151606,
                                    "committed_total": 1385910272,
                                    "populated_private": 0,
                                    "populated_scaled": 411303842.98151606,
                                    "populated_total": 1385910272,
                                }
                            ],
                        }
                    ],
                    "undigested": 0,
                    "digest": {
                        "buckets": [
                            {
                                "name": "ZBI Buffer",
                                "populated_size": 0,
                                "committed_size": 0,
                            },
                            {
                                "name": "[Addl]Graphics",
                                "committed_size": 49152,
                                "populated_size": 65432,
                            },
                        ]
                    },
                }
            },
        )
