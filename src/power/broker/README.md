# power-broker

## Status: DRAFT

This contains a prototype implementation of Power Broker for the purposes of
testing and iterating upon the protocols proposed by go/fuchsia-power-broker .
These protocols should be very much considered a work-in-progress at this point
and the eventual design will be driven by the findings here.

## Building

To add this component to your build, append
`--with-base src/power/broker`
to the `fx set` invocation.

## Running

Use `ffx component run` to launch this component into a restricted realm
for development purposes:

```
$ ffx component run /core/ffx-laboratory:power-broker fuchsia-pkg://fuchsia.com/power-broker#meta/power-broker.cm
```

## Testing

To add tests to your build, append
`--with //src/power/broker:tests`
to the `fx set` invocation.

Unit tests can be run with:

```
$ fx test power-broker-unittests
```

Integration tests can be run with:

```
fx test //src/power/broker/tests/integration
```

