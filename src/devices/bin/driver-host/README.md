# driver-host

This is the rust implementation of the elf component which drivers are loaded into. It's primary duties are related to managing the driver runtime as well as managing the driver lifecycle. 

## Building

To add this component to your build, append
`--with-base src/devices/bin/driver-host`
to the `fx set` invocation.

## Running

Use `ffx component run` to launch this component into a restricted realm
for development purposes:

```
$ ffx component run /core/ffx-laboratory:driver-host fuchsia-pkg://fuchsia.com/driver-host#meta/driver-host.cm
```

## Testing

Unit tests for driver-host are available in the `driver-host-tests`
package.

```
$ fx test driver-host-tests
```

