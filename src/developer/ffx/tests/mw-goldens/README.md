# MachineWriter schema golden file tests

These tests are used to detect changes in the schema of the JSON output
for ffx commands. The JSON output is also called "machine output". To get
the machine output for a ffx command use the `--machine` top-level option
with ffx. For example:

`ffx --machine json-pretty target list`

To generate the schema for the output use the `--schema` top-level option:

`ffx --schema --machine json-pretty target list`

These tests take a list of commands that the schema should be monitored for
changes, and uses golden_files tests to detect changes.

