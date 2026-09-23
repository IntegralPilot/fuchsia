# Copyright 2022 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

import argparse
import os
import sys

CML = """{{
    include: [ "//src/sys/test_runners/inspect/default.shard.cml" ],
    program: {{
        accessor: "ALL",
        timeout_seconds: "60",
        cases: [
          "bootstrap/archivist:root/fuchsia.inspect.Health:status WHERE [s] s == 'OK'",
          "{filtering_enabled_selector}",
          "{count_selector}",
          {selectors}
        ],
    }},
}}
"""

FILTERING_ENABLED_SELECTOR = "bootstrap/archivist:root/pipelines/{pipeline_name}:filtering_enabled WHERE [s] {value}"

# This selector checks that the number of selectors we see configured are greater or equal to the
# number of selector files we got. This should be a good enough signal to know that the pipeline is
# configured correctly. Further checking would require parsing the selector files, deduping, etc.
COUNT_SELECTOR = "bootstrap/archivist:root/pipelines/{pipeline_name}:selector_count WHERE [s] s >= {count}"

CONFIG_FILE_SELECTOR = "bootstrap/archivist:root/pipelines/{pipeline_name}/config_files/{file_name}"


def path_to_selector(pipeline_name: str, path: str) -> str | None:
    """
    Creates an archivist selector for the given pipeline and selector config file.

    Args:
        pipeline_name: The name of the privacy pipeline.
        path: A path to a config file in that pipeline.

    Returns:
        A selector to verify that config health in the archivist inspect.
    """
    file, ext = os.path.splitext(os.path.basename(path))
    if ext != ".cfg":
        return None
    return CONFIG_FILE_SELECTOR.format(
        pipeline_name=pipeline_name, file_name=file
    )


def run(
    pipeline_name: str,
    files: list[str],
    output_path: str,
    expect_disabled: bool,
) -> None:
    selectors_list = [
        f'"{selector}"'
        for path in files
        if (selector := path_to_selector(pipeline_name, path)) is not None
    ]
    count = len(selectors_list)
    selectors = ",".join(selectors_list)

    count_selector = COUNT_SELECTOR.format(
        pipeline_name=pipeline_name, count=count
    )

    value = "Not(s)" if expect_disabled else "s"
    filtering_enabled_selector = FILTERING_ENABLED_SELECTOR.format(
        pipeline_name=pipeline_name, value=value
    )

    cml = CML.format(
        selectors=selectors,
        count_selector=count_selector,
        filtering_enabled_selector=filtering_enabled_selector,
    )

    with open(output_path, "w") as f:
        f.write(cml)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Process the given selector pipeline files into selectors for Inspect."
    )
    parser.add_argument(
        "-f", "--file", action="append", help="Selector file", default=[]
    )
    parser.add_argument("-n", "--name", help="The pipeline name", required=True)
    parser.add_argument(
        "-o",
        "--out",
        help="The path where the cml will be written",
        required=True,
    )
    parser.add_argument(
        "-d",
        "--expect-disabled",
        action="store_true",
        help="If set, expect the pipeline to be disabled",
    )
    args = parser.parse_args()
    run(args.name, args.file, args.out, args.expect_disabled)
    return 0


if __name__ == "__main__":
    sys.exit(main())
