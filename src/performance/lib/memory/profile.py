# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Supports using `ffx profile memory` in e2e tests."""

import fnmatch
import json
import re
from typing import Any, Mapping, cast

from reporting import metrics

_DESCRIPTION_BASE = "Total populated bytes for private uncompressed memory VMOs"


def capture(
    dut: Any,
    principal_groups: Mapping[str, str] | None = None,
    board_memory: int = 2 * 1024**3,
    buckets_metrics: str | None = None,
) -> metrics.Report:
    """Captures kernel and user space memory metrics using `ffx profile memory`.
    See documentation at
    https://fuchsia.dev/fuchsia-src/development/tools/ffx/workflows/explore-memory-usage

    Args:
      dut: A FuchsiaDevice instance connected to the device to profile.
      principal_groups: Mapping from group name to a `fnmatch` pattern
        that selects the principals by name. A metric labelled
        "Memory/Principal/{group_name}/PrivatePopulated" is returned for each
        item.
      board_memory: Total RAM of the board in bytes, used to calculate Fuchsia
        OS populated memory.
      buckets_metrics: For each bucket matching this regular expression
        a metric labelled "Memory/Bucket/{bucket_name}/CommittedBytes" is
        returned. When not set, none is returned.

    Returns:
      metrics.Report containing two sets of memory measurements:
        - "Memory/Process/{starnix_kernel,binder}.PrivatePopulated": total populated
            bytes for VMOs. This is private uncompressed memory.
        - A whole-device memory digest, as retrieved by `ffx profile memory`.

    """
    principal_groups = principal_groups or {}

    component_profile = json.loads(
        dut.ffx.run(
            [
                "--machine",
                "json",
                "profile",
                "memory",
                "--buckets",
                "--backend",
                "memory_monitor_2",
            ],
        )
    )
    structured = process_component_profile(
        principal_groups, buckets_metrics, component_profile, board_memory
    )
    description = metrics.describe_callable(capture)
    return metrics.Report(
        structured,
        {
            "memory_profile": _simplify_digest(component_profile),
        },
        [
            metrics.MetricsProcessorDescription(
                doc=description["doc"],
                code_path=description["code_path"],
                line_no=description["line_no"],
                metrics=[tcr.describe() for tcr in structured],
            )
        ],
    )


def process_component_profile(
    principal_groups: Mapping[str, str],
    buckets_metrics: str | None,
    component_profile: Any,
    board_memory: int,
) -> list[metrics.TestCaseResult]:
    """Processes the component profile data to extract memory metrics.

    Args:
      principal_groups: Mapping from group name to a `fnmatch` pattern
        that selects the principals by name. A metric labelled
        "Memory/Principal/{group_name}/PrivatePopulated" is returned for each
        item.
      buckets_metrics: For each bucket matching this regular expression
        a metric labelled "Memory/Bucket/{bucket_name}/CommittedBytes" and
        "Memory/Bucket/{bucket_name}/PopulatedBytes" is returned. When not set,
        none is returned.
      component_profile: The JSON data extracted from `ffx profile memory`.
      board_memory: Total RAM of the board in bytes, used to calculate Fuchsia
        OS populated memory.

    Returns:
      A list of extracted memory metrics as TestCaseResult objects.
    """
    results: list[metrics.TestCaseResult] = []

    component_digest = component_profile["ComponentDigest"]
    if buckets_metrics:
        for bucket in component_digest["digest"]["buckets"]:
            if re.match(buckets_metrics, bucket["name"]):
                direction = (
                    metrics.Direction.biggerIsBetter
                    if bucket["name"]
                    in ("[Addl]PagerOldest", "[Addl]PagerTotal")
                    else None
                )
                results.append(
                    metrics.TestCaseResult(
                        label=f"Memory/Bucket/{cleanup_bucket_name(bucket['name'])}/CommittedBytes",
                        unit=metrics.Unit.bytes,
                        values=[bucket["committed_size"]],
                        doc=f"Total committed bytes in the bucket: {bucket['name']}",
                    )
                )
                results.append(
                    metrics.TestCaseResult(
                        label=f"Memory/Bucket/{cleanup_bucket_name(bucket['name'])}/PopulatedBytes",
                        unit=metrics.Unit.bytes,
                        values=[bucket["populated_size"]],
                        doc=f"Total populated bytes in the bucket: {bucket['name']}",
                        direction=direction,
                    )
                )

    for group_name, pattern in principal_groups.items():
        private_populated = sum(
            principal["populated_private"]
            for principal in component_digest["principals"]
            if fnmatch.fnmatch(principal["name"], pattern)
        )
        results.append(
            metrics.TestCaseResult(
                label=f"Memory/Principal/{group_name}/PrivatePopulated",
                unit=metrics.Unit.bytes,
                values=[private_populated],
                doc=f"{_DESCRIPTION_BASE}: {group_name}",
            )
        )

    if (
        "kernel" in component_digest
        and "memory_statistics" in component_digest["kernel"]
    ):
        total_bytes = component_digest["kernel"]["memory_statistics"][
            "total_bytes"
        ]

        addl_anon_bytes = 0
        starnix_bytes = 0
        eng_tools_bytes = 0

        for bucket in component_digest["digest"]["buckets"]:
            name = bucket["name"]
            if name == "[Addl]PopulatedAnonymousBytes":
                addl_anon_bytes = bucket["populated_size"]
            elif name == "StarnixContainer":
                starnix_bytes = bucket["populated_size"]
            elif name == "EngTools":
                eng_tools_bytes = bucket["populated_size"]

        # Total Fuchsia OS populated memory is computed as follows:
        # 2 GiB (total RAM) - total_bytes (all memory addressable by the kernel and user space):
        #   this gives the memory that is held by the bootloaders, firmware, and other reservations
        # + addl_anon_bytes: all the non-reclaimable memory on the system
        # - starnix_bytes: memory of processes running under Starnix (product memory)
        # - eng_tools_bytes: memory of engineering tooling (testing overhead)
        fuchsia_os_memory_bytes = max(
            0,
            board_memory
            - total_bytes
            + addl_anon_bytes
            - starnix_bytes
            - eng_tools_bytes,
        )

        results.append(
            metrics.TestCaseResult(
                label="Memory/System/FuchsiaOSPopulatedBytes",
                unit=metrics.Unit.bytes,
                values=[fuchsia_os_memory_bytes],
                doc="Fuchsia OS Populated Memory",
            )
        )

    return results


def cleanup_bucket_name(name: str) -> str:
    """Makes buckets compliant with metric system naming conventions.

    Refer to src/performance/lib/perf_publish/publish.py for more details.
    """
    result, _count = re.subn(r"\W", "", name)
    return result


def _simplify_name_to_vmo_memory(
    name_to_vmo_memory: metrics.JSON,
) -> list[metrics.JSON]:
    """Prepares `ffx profile memory` JSON data for BigQuery.

    Input sample:
        {
            "[blobs]": {
                "private": 0,
                "private_populated": 0,
                "scaled": 296959744,
                "scaled_populated": 296959744,
                "total": 871759872,
                "total_populated": 871759872,
                "vmos": [
                    153096,
                    127992
                ]
            }
        }

    Output sample:
        {
            "name": "[blobs]",
            "private": 0,
            "private_populated": 0,
            "scaled": 296959744,
            "scaled_populated": 296959744,
            "total": 871759872,
            "total_populated": 871759872,
        }
    """
    if not isinstance(name_to_vmo_memory, dict):
        raise ValueError
    return [
        dict(name=cast(metrics.JSON, k)) | _with_vmos_removed(v)
        for k, v in name_to_vmo_memory.items()
    ]


def _with_vmos_removed(metrics_dict: metrics.JSON) -> dict[str, metrics.JSON]:
    """Returns a copy of the specified directory without the "vmos" key."""
    if not isinstance(metrics_dict, dict):
        raise ValueError
    return {k: v for k, v in metrics_dict.items() if k != "vmos"}


def _simplify_principal(principal: metrics.JSON) -> dict[str, metrics.JSON]:
    """Prepares `ffx profile memory component` JSON data for BigQuery."""
    if not isinstance(principal, dict):
        raise ValueError
    return principal | {"vmos": _simplify_name_to_vmo_memory(principal["vmos"])}


def _simplify_principals(principals: metrics.JSON) -> list[metrics.JSON]:
    """Prepares `ffx profile memory` JSON data for BigQuery.

    Turns a list of principals into a list of simplified processes.
    """
    if not isinstance(principals, list):
        raise ValueError
    return [_simplify_principal(b) for b in principals]


def _simplify_digest(component_profile: metrics.JSON) -> metrics.JSON:
    result = {}

    if isinstance(component_profile, dict):
        digest = component_profile["ComponentDigest"]
        if not isinstance(digest, dict):
            raise ValueError
        result = digest | {
            "principals": _simplify_principals(digest["principals"])
        }

    return result
