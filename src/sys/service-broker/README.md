# service-broker

This component provides a default broker implementation capable of common simple policies.
It is meant to be repurposable throughout the component hierarchy.

## Building

This component is provided on all products except those derived from embeddable.

## Running

Use `ffx component run` to launch this component into a restricted realm
for development purposes:

```
$ ffx component run /core/ffx-laboratory:service-broker fuchsia-pkg://fuchsia.com/service-broker#meta/service-broker.cm
```

## Testing

Unit tests for service-broker are available in the `service-broker-tests`
package.

```
$ fx test service-broker-tests
```

