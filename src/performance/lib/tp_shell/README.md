# Perfetto Trace Processor Wrapper Library (`tp_shell`)

A high-performance, persistent RPC-based Python wrapper around the official
Perfetto Trace Processor SDK.

This library packages the prebuilt host `trace_processor_shell` binary
hermetically as a module resource. Using the Perfetto python Library,
it starts the shell as a background daemon and executes SQL queries over a RPC
connection which sends and receives protobuf messages. This ensures the trace
is ingested and parsed only once, improving the performance for scripts
executing multi-query analysis.

Implementation notes of interest:

* This library handles the incompatibilities between the Perfetto GN
`python_library` rules and the Fuchsia `python_library` rules implementation
by referencing the source files from the perfetto source in this BUILD.gn  file.

* This library implements `FuchsiaPlatformDelegate` which is the Fuchsia specific
implementation of Pefetto's `PlatformDelegate`. This allows us to use the prebuilt
`trace_processor_shell` included in-tree vs. the default behavior of trying to
download it from the cloud and cache it.

* Also included internal to the library are the compiled protobufs needed to make
the RPC calls to `trace_processor_shell`. These are internal to the library and
users of the library do not need to interact with protobufs directly.

* The library includes custom `HttpUriResolver` and `HttpsUriResolver` implementations
registered with `FuchsiaPlatformDelegate`. This allows `PerfettoTraceProcessor` to directly
accept `ui.perfetto.dev` permalinks and direct HTTP/HTTPS URLs as input, resolving metadata
and streaming traces over the network without requiring local download steps.

* The library includes a copy of the `trace_processor_shell` so if the path to a
local copy is not specified, the library will use the bundled copy. This is
useful for agent skills and use outside the Fuchsia build directory.

---

## 1. Adding to `BUILD.gn`

To use this library in your Python tool or library, add the target dependency `//src/performance/lib/tp_shell` to your `BUILD.gn` file.

### Example: For a Python Library
```gn
import("//build/python/python_library.gni")

python_library("my_perf_library") {
  source_root = "."
  sources = [
    "analyzer.py",
  ]
  # Depend on tp_shell to make it importable at runtime
  library_deps = [
    "//src/performance/lib/tp_shell",
  ]
  enable_mypy = true
}
```

### Example: For a Python Binary / CLI Tool
```gn
import("//build/python/python_binary.gni")

python_binary("my_perf_tool") {
  main_source = "bin/main.py"
  deps = [
    "//src/performance/lib/tp_shell",
  ]
  enable_mypy = true
}
```

---

## 2. Using in Python

Import the `PerfettoTraceProcessor` class and instantiate it. When packaged correctly via `BUILD.gn`, the library is completely **zero-configuration** and automatically resolves its bundled prebuilt binary.

### Standard Context Manager Pattern (Recommended)
Using the context manager ensures that the background HTTPD server is cleanly shut down as soon as the `with` block exits.

```python
from tp_shell import PerfettoTraceProcessor

def analyze_trace(trace_path: str) -> None:
    # Zero-configuration: binary is resolved automatically from packaged resources
    with PerfettoTraceProcessor(trace_path) as tp:
        # Run a query: returns a list of dictionaries where keys are column names
        query = """
            SELECT name, dur / 1000000.0 as dur_ms
            FROM slice
            WHERE category = 'benchmark'
            ORDER BY dur DESC
            LIMIT 5
        """
        results = tp.run_query(query)

        for row in results:
            print(f"Slice: {row['name']} took {row['dur_ms']:.2f} ms")
```

### Direct Instantiation
If you cannot use a `with` statement, you must call `.close()` explicitly to terminate the background subprocess.

```python
from tp_shell import PerfettoTraceProcessor

tp = PerfettoTraceProcessor("path/to/trace.fxt")
try:
    results = tp.run_query("SELECT count(*) as cnt FROM slice")
    print(f"Total slices: {results[0]['cnt']}")
finally:
    # Always close to prevent zombie background daemons
    tp.close()
```

### Remote Permalinks and URLs
`PerfettoTraceProcessor` natively accepts `ui.perfetto.dev` permalinks and direct HTTP/HTTPS URLs as input. The trace metadata is resolved and streamed directly over the network without requiring a local `.fxt` download step or disk storage:

```python
from tp_shell import PerfettoTraceProcessor

# Pass a ui.perfetto.dev permalink or HTTP/HTTPS trace URL directly
permalink = "https://ui.perfetto.dev/#!/?s=0fdf12cb1d57131188fd93b6cd857865156e8f5b"
with PerfettoTraceProcessor(permalink) as tp:
    results = tp.run_query("SELECT count(1) as slice_count FROM slice")
    print(f"Total slices in permalink: {results[0]['slice_count']}")
```

---

## 3. Explicit Binary Overrides

By default, the library resolves its bundled resource. If you need to test with a custom version of the `trace_processor_shell` executable, you can pass the path explicitly:

```python
tp = PerfettoTraceProcessor(
    trace_path="path/to/trace.fxt",
    tp_shell_path="/path/to/custom/trace_processor_shell"
)
```

If the binary cannot be resolved from either packaged resources or an explicit path, a `FileNotFoundError` will be raised.
