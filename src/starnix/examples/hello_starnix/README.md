# Hello Starnix

This directory contains a simple Linux binary that runs in Starnix.

## How to run hello_starnix

### Build and run Fuchsia

Build the `workbench_eng.x64` product with this example and its container:

```sh
fx set workbench_eng.x64 \
    --with //src/starnix/examples/hello_starnix \
    --with //src/starnix/containers/empty_container
fx build
```

Run this product as usual. For example, by running `fx serve` in one terminal and
`ffx emu start --headless --console --net tap` in another terminal.

### Boot a container

To run the `hello_starnix` component, we first need to boot a container. We use
`empty_container.cm`, which does not have a disk image. Instead, the empty container
mounts its own package as its root file system.  This container is useful for running
self-contained binaries (e.g., that do not require `libc`):

```sh
ffx component resolve core/starnix_runner
ffx component run core/starnix_runner/playground:starless fuchsia-pkg://fuchsia.com/starless#meta/empty_container.cm
```

You can confirm that this container is running using `ffx component list`. If everything
has gone well, you should see `core/starnix_runner/playground:starless` running in
the system.

### Run hello_starnix

Create the `hello_starnix` component inside this container:

```sh
ffx component create core/starnix_runner/playground:starless/daemons:hello_starnix fuchsia-pkg://fuchsia.com/hello_starnix#meta/hello_starnix.cm
```

You should see some output similar to the following in `ffx log`:

```
[00203.847301][kernels:empty_container.cm_3cT3bre][9:9[hello_starnix],stdio,starnix] INFO: hello starnix
```
