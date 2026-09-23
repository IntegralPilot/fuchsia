# Power Framework Examples

Examples that demonstrate power concepts.

Get started by adding power examples to your `fx args`:

```
fx set core.x64 --with //examples/power --with-test //examples/power:hermetic_tests
```

Make sure to run `fx build` and `fx serve` if it's your first time setting up these examples.

You can then run all example tests using `fx test`.

## Power Topology

Concepts from https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0250_power_topology.

Examples below include recommended unit and integration tests.

## State Recorder

Record timeseries of state transitions to Inspect and trace using a standardized
format.

### Taking a Wake Lease

Prevent system suspend by requesting a wake lease from the System Activity Governor.
