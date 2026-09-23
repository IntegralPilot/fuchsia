# Configure and build Fuchsia {#configure-and-build-fuchsia}

This guide provide instructions on how to configure and build Fuchsia
on a host machine.

The steps are:

1. [Prerequisites](#prerequisites).
1. [Set your build configuration](#set-your-build-configuration).
1. [Speed up the build (Optional)](#speed-up-the-build).
1. [Build Fuchsia](#build-fuchsia).

## Prerequisites {#prerequisites .numbered}

This guide requires the following prerequisite items:

* [Source code setup](#source-code-setup)
* [Hardware requirements](#hardware-requirements)

### Source code setup {#source-code-setup}

**Complete the [Download the Fuchsia source code][get-fuchsia-source] guide.**
This guide helps you download the Fuchsia source code and set up the Fuchsia
development environment on your host machine.

### Hardware requirements {#hardware-requirements}

You can build Fuchsia on a host machine with the x86-64 Linux architecture
(Debian-based distributions only).

**Windows is not supported.**

## Set your build configuration {#set-your-build-configuration .numbered}

Fuchsia's build configuration informs the build system which product to
build and which architecture to use.

To set your Fuchsia build configuration, run the following
[`fx set`][fx-set-reference] command:

```posix-terminal
fx set {{ '<var>' }}PRODUCT{{ '</var>' }}.{{ '<var>' }}BOARD{{ '</var>' }}
```

Replace the following:

* `PRODUCT`: The Fuchsia product that you want to build; for example, `core` and
  `workbench_eng`.
* `BOARD`: The board configuration; for example, `x64` or `qemu-arm64`.

Important: The generic `arm64` board configuration does not support running
in the emulator because it is configured for physical target hardware. To build
a Fuchsia image for the emulator on an ARM64 host, or to emulate an ARM64
target, you must use the `qemu-arm64` board configuration (for example,
`terminal.qemu-arm64`).

The example command below sets the build configuration to `core.x64`:

```posix-terminal
fx set core.x64
```

  * `core` is a product with the minimum feature set of Fuchsia, which includes
    network capabilities.
  * `x64` is a board that can run on a wide range of x64 devices, including the
    Fuchsia emulator ([FEMU][femu]).

On the other hand, the example below sets the build configuration to
[`workbench_eng.x64`][build-workbench]:

```posix-terminal
fx set workbench_eng.x64
```

The lists of possible [boards][boards] and [products][products] are aggregated
from the root of the Fuchsia source repository. For more information, see
[Configure a build][configure-a-build].

## Speed up the build (Optional) {#speed-up-the-build .numbered}

Note: This step is **not required** for building Fuchsia.

To accelerate the Fuchsia build locally, use [`ccache`][ccache]{:.external}
to cache C and C++ artifacts from previous builds.

Install the `ccache` package:

```posix-terminal
sudo apt install ccache
```

`ccache` is enabled automatically if your `CCACHE_DIR` environment variable
refers to an existing directory.

To override this default behavior, specify the following flags to `fx set`:

*   Force the use of `ccache` even when other accelerators are available:

    <pre class="prettyprint">
    <code class="devsite-terminal">fx set <var>PRODUCT</var>.<var>BOARD</var> --ccache</code>
    </pre>

*   Disable the use of `ccache`:

    <pre class="prettyprint">
    <code class="devsite-terminal">fx set <var>PRODUCT</var>.<var>BOARD</var> --no-ccache</code>
    </pre>

## Build Fuchsia {#build-fuchsia .numbered}

The [`fx build`][fx-build-reference] command executes the build to transform
source code into packages and other build artifacts.

To build Fuchsia, run the following command:

```posix-terminal
fx build
```

Note: Building Fuchsia can take up to 90 minutes.

When you modify source code, run the `fx build` command again to perform an
incremental build, or run the `fx -i build` command to start a watcher, which
automatically builds whenever you update the source code.

For more information on building Fuchsia,
see [Execute a build](/docs/development/build/fx.md#execute-a-build).

## Next steps

To launch the Fuchsia emulator (FEMU) on your machine, see
[Start the Fuchsia emulator](/docs/get-started/set_up_femu.md).

However, if you want to run Fuchsia on a hardware device, see
[Install Fuchsia on a device](/docs/development/hardware/README.md) instead.

<!-- Reference links -->

[get-fuchsia-source]:/docs/get-started/get_fuchsia_source.md
[build-workbench]: /docs/development/build/build_workbench.md
[fx-set-reference]: https://fuchsia.dev/reference/tools/fx/cmd/set
[fx-build-reference]: https://fuchsia.dev/reference/tools/fx/cmd/build
[femu]: /docs/get-started/set_up_femu.md
[boards]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/boards/
[products]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/products/
[configure-a-build]: /docs/development/build/fx.md#configure-a-build
[ccache]: https://ccache.dev/
