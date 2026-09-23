#!/usr/bin/env fuchsia-vendored-python
# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""
Returns graphics metrics in a JSON file.

# Prerequisites:

* Ensure that you have the label
`//src/performance/lib/trace_processing:run_graphics_metrics` in your
`host_labels` in `$(fx get-build-dir)/args.gn`:

    ```
    host_labels = [
      "//src/performance/lib/trace_processing:run_graphics_metrics",
    ]
    ```
* `fx build`

# Run

Run as:

```
fx run_graphics_metrics \
    path/to/json_or_fxt/file \
    path/to/output/file.fuchsiaperf.json
```

# Print the output

Example below.

```
$ jq -r \
  '["Label", "Test-Suite", "Value", "Unit"], (.[] | [.label, .test_suite, .values[0], .unit]) | @tsv' \
  $HOME/tmp/foo/some_file.fuchsiaperf.json \
  | column -t
Label               Test-Suite  Value               Unit
RenderCpuP5         Manual      0.9867073000000001  milliseconds
RenderCpuP25        Manual      1.044023            milliseconds
RenderCpuP50        Manual      1.0817964999999998  milliseconds
RenderCpuP75        Manual      1.12369725          milliseconds
RenderCpuP95        Manual      15.853793750000005  milliseconds
RenderCpuMin        Manual      0.908333            milliseconds
RenderCpuMax        Manual      33.499375           milliseconds
RenderCpuAverage    Manual      2.627292306763285   milliseconds
RenderTotalP5       Manual      4.0638462           milliseconds
RenderTotalP25      Manual      5.30674475          milliseconds
RenderTotalP50      Manual      7.4009115           milliseconds
RenderTotalP75      Manual      20.3377085          milliseconds
RenderTotalP95      Manual      24.368351650000008  milliseconds
RenderTotalMin      Manual      3.313125            milliseconds
RenderTotalMax      Manual      49.200417           milliseconds
RenderTotalAverage  Manual      11.583923586956521  milliseconds
```

"""

# Almost entirely copied from `run_suspend_metrics.py`.
import argparse
import pathlib
import sys
from typing import Any, Tuple

from reporting import metrics
from trace_processing import trace_importing, trace_model
from trace_processing.metrics import scenic


def PrintClientInfo(model: trace_model.Model) -> None:
    """
    Prints all of the client information for the GPU.
    """

    def FindProcessAndThread(
        model: trace_model.Model, thread_id: int
    ) -> Tuple[Any, Any]:
        for process in model.processes:
            for thread in process.threads:
                if thread.tid == thread_id:
                    return (process, thread)
        return (None, None)

    gpu_process: Any = next(
        filter(lambda p: "#gpu" in p.name, iter(model.processes))
    )
    for thread in gpu_process.threads:
        words = thread.name.split()
        if words[0] != "ConnectionThread":
            continue
        client_thread = int(words[1])
        process, thread = FindProcessAndThread(model, client_thread)
        if process:
            print(f"Connection ID: {client_thread}")
            print(f"    Process: {process.name}")
            print(f"    Thread: {thread.name}")


def main() -> None:
    """
    Takes in a trace file in JSON format and writes metrics in fuchsiaperf.json
    format to `output_path`.
    """
    parser = argparse.ArgumentParser()
    parser.add_argument("path_to_trace", type=str)
    parser.add_argument("output_path", type=str)
    parser.add_argument(
        "--clients", action="store_true", help="Print GPU client info"
    )
    args = parser.parse_args()

    if args.path_to_trace.endswith(".json"):
        path_to_trace_json = args.path_to_trace
    elif args.path_to_trace.endswith(".fxt"):
        trace2json_path = pathlib.Path(sys.argv[0]).parent / "trace2json"
        path_to_trace_json = trace_importing.convert_trace_file_to_json(
            trace_path=args.path_to_trace,
            trace2json_path=trace2json_path,
        )
    else:
        raise Exception("trace must be in either fxt or json format")

    model: trace_model.Model = trace_importing.create_model_from_file_path(
        path_to_trace_json
    )
    if args.clients:
        PrintClientInfo(model)
    trace_results = scenic.ScenicMetricsProcessor().process_metrics(model)

    metrics.TestCaseResult.write_fuchsiaperf_json(
        results=trace_results,
        test_suite="Manual",
        output_path=pathlib.Path(args.output_path),
    )


if __name__ == "__main__":
    main()
