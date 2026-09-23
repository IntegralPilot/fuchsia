# Hello Debian

This directory contains simple Linux binaries that runs in a Debian container.
The Debian container is useful for running binaries that require `libc`.

There are two example binaries, written in C++ and Rust respectively.

## How to run hello_debian

### Build and run Fuchsia

Build the `workbench_eng.x64` product with this example:

```sh
fx set workbench_eng.x64 \
    --with //src/starnix/examples/hello_debian \
    --with //src/starnix/containers/debian
```

Run this product as usual. For example, by running `fx serve` in one terminal and
`ffx emu start --headless --console --net tap` in another terminal.

### Boot a container

To run a `hello_debian` component, we first need to boot the `debian` container:

```sh
ffx component resolve core/starnix_runner
ffx component run core/starnix_runner/playground:debian fuchsia-pkg://fuchsia.com/debian#meta/debian_container.cm
```

You can confirm that this container is running using `ffx component list`. If everything
has gone well, you should see `core/starnix_runner/playground:debian` running in
the system.

### Run hello_debian

Create a `hello_debian` component inside this container:

- C++:

    ```sh
    ffx component create core/starnix_runner/playground:debian/daemons:hello_debian fuchsia-pkg://fuchsia.com/hello_debian_cpp#meta/hello_debian_cpp.cm
    ```

- Rust:

    ```sh
    ffx component create core/starnix_runner/playground:debian/daemons:hello_debian fuchsia-pkg://fuchsia.com/hello_debian_rust#meta/hello_debian_rust.cm
    ```

You should see some output similar to the following in `ffx log`:

```
[00061.161627][kernels:JNfdNLC][5:5[hello_debian_cpp],stdio,starnix] INFO: hello debian
```
