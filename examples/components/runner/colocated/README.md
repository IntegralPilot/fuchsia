# Colocated runner example

This is an example of how to implement a component runner where the components
being run are colocated inside the runner process, i.e. these components share
an address space.

## What does this runner do

The `colocated` runner demonstrates how to attribute memory to each component
it runs. A program run by the `colocated` runner will create and hold a VMO.
This VMO will be reported by the runner as part of the memory attribution
protocol.

If the program is started with a `PA_USER0` numbered handle, it will serve the
`fuchsia.examples.colocated.Colocated` protocol over this channel, which can be
used to retrieve the koid of the VMO created by the colocated component, from
the component itself.

## Program schema

The manifest of a component run by this runner would look like the following:

```json5
{
    program: {
        // The `colocated` runner should be registered in the environment.
        runner: "colocated",

        // Size of the VMO in bytes. It will be rounded up to the page size.
        vmo_size: "4096",
    }
}
```

## Building

See [Building Hello World Components](/examples/hello_world/README.md#building).

## Running

To experiment with the `colocated` runner, provide this component
URL to `ffx component run`:

```bash
$ ffx component run /core/ffx-laboratory:colocated-runner-example 'fuchsia-pkg://fuchsia.com/colocated-runner-example#meta/colocated-runner-example.cm'
```

This URL identifies a realm that contains a colocated runner, as well as a
collection for running colocated components using that runner.

To run a colocated component, provide this URL to `ffx component run`:

```bash
$ ffx component run /core/ffx-laboratory:colocated-runner-example/collection:1 'fuchsia-pkg://fuchsia.com/colocated-runner-example#meta/colocated-component.cm'
```

This will start a component that lives inside the address space of the
`colocated` runner.

You may replace `collection:1` with `collection:2` etc. to start multiple
colocated components in this collection.

Stopping a colocated component will free up the associated memory.

