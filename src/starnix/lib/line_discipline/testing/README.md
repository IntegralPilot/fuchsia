# Line Discipline Trace Generator

This directory contains `trace_generator.py`, a utility for generating golden
trace files on Linux to verifying Starnix line discipline behavior.

## Usage

The generator MUST be run on a Linux host (not Fuchsia/Starnix) as it captures
the behavior of the host's PTY implementation.

### Generating Traces

To generate all trace files:

```bash
python3 src/starnix/lib/line_discipline/testing/trace_generator.py
```

This will generate trace files in `src/starnix/lib/line_discipline/testing/generated/`.

> **Note:** You must run this script manually whenever you change or add a scenario.

## Adding New Tests

1.  Add the a scenario to the `scenarios` directory.
2.  Add the the name of the scenario to `scenarios_list.json`.
3.  Run the generator to produce the JSON trace.
