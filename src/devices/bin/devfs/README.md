# devfs

This component exists solely to provide the correct dependency structure to the
component framework so that, during shutdown, when all components that depend
on devfs are removed, the driver manager can be signalled that driver shutdown
can commence.

In prod, devfs is a "builtin component" that runs in component manager's
process. This saves on the overhead of running a small component. A standalone
version is provided for integration tests (devfs-test.cml).

## Building

This component should automatically be included in most builds.
To add this component to your build, append
`--with-base src/devices/bin/devfs`
to the `fx set` invocation.

## Running

Use `ffx component run` to launch this component into a restricted realm
for development purposes:

```
$ ffx component run /core/ffx-laboratory:devfs fuchsia-pkg://fuchsia.com/devfs#meta/devfs.cm
```
