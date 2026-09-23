# Fuchsia build system

The Fuchsia build system aims at building both boot images and updatable
packages for various devices. The Fuchsia build system uses [GN][gn-main], a
meta-build system that generates build files consumed by [Ninja][ninja-main],
which executes the actual build.

> **Note:**
> Most developers should interact with the build system through [`fx` workflows](fx.md)
> (such as `fx build` and `fx set`) rather than running GN, Ninja, or Bazel directly
> (even via wrappers like `fx gn` or `fx ninja`).


Note: Zircon uses a different build system, but it also uses GN and
Ninja.

## Concepts

If you are unfamiliar with Fuchsia's build system and GN, see [Using GN
build][gn-preso], which outlines the basic principles of the GN build system.

The following sections cover several concepts around Fuchsia's build system.

### Build configuration

The contents of the generated Fuchsia images are controlled by a build
configuration, which is a combination of a board and a product.
**Boards and products are build targets that define the packages and
dependencies** that are included in images. For more information on the
structure and usage of these build configurations, see
[Build configuration](software_assembly/build_configuration.md).

### Build targets

Build targets are defined in `BUILD.gn` files that exist throughout the source
tree. These files use a Python-like syntax to declare buildable objects.
For example:

```py
import("//build/some/template.gni")

my_template("foo") {
  name = "foo"
  extra_options = "//my/foo/options"
  deps = [
    "//some/random/framework",
    "//some/other/random/framework",
  ]
}
```

Available commands (invoked through the `gn` cli tool) and constructs
built-in target declaration types are defined in the
[GN reference][gn-reference]. There are also a handful of custom templates in
`.gni` files in the [`//build` project][build-project].

Fuchsia defines many [custom templates](/docs/development/components/build.md#gn_templates)
to support defining and building Fuchsia specific artifacts.

### Build optimization flags {:#build-optimization-flags}

When building Fuchsia using `fx set`, you can specify a build optimization flag
to control the trade-off between compilation time, runtime performance, and
debuggability. The build optimization flags are `--debug`, `--balanced`, and
`--release`.

Choosing the right flag can significantly impact your development workflow,
build times, and the performance characteristics of the resulting image.

#### Quick comparison

Note: For full details, see [Full comparison of build optimization flags].

|  | `--debug` | `--balanced` | `--release` |
| :---- | :---- | :---- | :---- |
| **Primary focus** | Debug assertions, optimized for use with the debugger | Compile speed and good runtime performance. | Max runtime performance and smallest size |
| **Compile time** | Faster (Incremental) | Medium (2-4x faster than release for some targets) | Slower |
| **Runtime performance** | Slower | Good (acceptable for most development) | Faster |
| **Binary size** | Larger | Much smaller than debug, and slightly larger than release | Smaller |
| **Optimizations** | Minimal | Some | Full |
| **Debug experience** | Full | Between `debug` and `release` (slightly less debuggability than debug) | Minimal |
| **Recommended for** | Active coding and debugging | Daily development and faster iterations | Production, benchmarking, performance analysis, final validation |

#### Set the compilation mode

Append the desired flag to your `fx set` command:

```shell
fx set PRODUCT.BOARD [--debug | --balanced | --release]
```

For example:

* `fx set core.x64 --debug`
* `fx set core.x64 --balanced`

#### Why `--balanced`?

The `--balanced` flag was introduced to address the significant compilation time
overhead of `--release` builds, especially for large Rust and C++ targets. By
selectively enabling optimizations and using faster alternatives like ThinLTO
(instead of Full LTO) for C++ and more codegen units (i.e. threads at compile
time) for Rust, `--balanced` offers a better developer experience for tasks
requiring better-than-debug performance.

As Fuchsia evolves, `--release` builds may incorporate even more aggressive
(and potentially slower to compile) optimizations like Rust Full LTO, PGO and
higher optimization levels for performance-critical binaries. On the other hand,
if you use `--balanced` this will allow you to do performance-aware development
so that you can benefit from ongoing compile-time improvements while maintaining
good runtime characteristics.

#### Full comparison of build optimization flags {:#full-comparison-of-build-optimization-flags}

This section does a full comparison between the build optimization flags:

* [--debug](#--debug)
* [--balanced](#--balanced)
* [--release](#--release)

##### `--debug` {#--debug}

* **Primary goal:** Faster incremental compilation, full debuggability.
* **Optimizations:** Minimal to none. Code is compiled to be as close to the
  source as possible.
* **Debug experience:** Full debug symbols are included.
* **Runtime performance:** Slower. Not suitable for performance testing or
  production.
* **Compile time (full rebuild):** Generally faster than `--release` and
  `--balanced` for initial builds due to lack of optimization passes.
  Incremental builds are typically the fastest.
* **When should you use this:**
  * Actively developing and debugging code.
  * You need to step through code with a debugger and inspect variables
    accurately.
  * Rapid iteration is more important than runtime performance.

##### `--balanced` {#--balanced}

Note: This optimization flag is recommended default for faster iteration with
good performance.

* **Primary goal:** A balance between compilation speed and runtime performance.
* **Optimizations:** A curated set of optimizations that provide good runtime
  performance without the excessive compile times of `--release`.
* **Debug experience:** Between `--debug` and `--release` slightly more
  debuggability than release. Some optimizations might make precise debugging
  harder than `--debug`.
* **Runtime performance:** Good. Slightly slower (potentially 10-20% in some
  areas) than a full `--release` build, but significantly faster than `--debug`.
  Performance is generally acceptable for most development and testing
  scenarios.
* **Compile time:** Significantly faster than `--release`. For large Rust
  targets, this can be **2-4x faster**.
  * For example: `netstack3` compiles 4x faster (70s vs 280s).
  * For example: `component_manager` compiles 2.6x faster (70s vs 180s).
* **When should you use this:**
  * **This should be your default compilation mode when you need something that
    runs faster than `--debug` but want to avoid the long compile times of
    `--release`.**
  * General development and iteration where `--debug` is too slow at runtime.
  * When you need to test features with reasonable performance without waiting
    for full release builds.
  * To benefit from ongoing compile-time improvements, as this mode is actively
    being optimized for speed.

##### `--release` {#--release}

* **Primary goal:** Maximum runtime performance and smallest binary size.
* **Optimizations:** Full optimizations are enabled. This includes aggressive
  techniques like:
  * Link-Time Optimization (LTO), often Full LTO.
  * Profile-Guided Optimization (PGO) where applicable.
  * Higher compiler optimization levels (e.g., `-O3`).
* **Debug experience:** Minimal. Debugging can be very challenging.
* **Runtime performance:** Fastest. This is the mode for benchmarking and
  production deployments.
* **Compile time:** Slowest, due to extensive optimization passes and LTO.
* **When should you use this:**
  * Building for production or deployment.
  * Running performance benchmarks.
  * You need the absolute smallest binary size and highest runtime speed, and
    are willing to accept long build times.

## Execute a build

The simplest way to execute a build is through the `fx` tool by using `fx set`
to [configure a build](#configure-build)
and then `fx build` as described in
[fx workflows](/docs/development/build/fx.md).

### Configure a build {:#configure-build}

Configure the primary build artifacts by choosing the board and product
to build:

* {fx set}

  ```posix-terminal
  fx set core.x64
  ```

  You can also set an optimization flag on this command. See
  [Build optimization flags](#build-optimization-flags). For example:

  ```posix-terminal
  fx set core.x64 --balanced
  ```

* {fx gn gen}

  ```posix-terminal
  fx gn gen $(fx get-build-dir) --args='import("//boards/x64.gni") import("//products/core.gni")'
  ```

  For a list of all GN build arguments, run:

  ```posix-terminal
  fx gn args $(fx get-build-dir) --list
  ```

This creates a build directory (usually `out/default`) that contains Ninja
and Bazel files.

### Generate a build {:#generate-build}

Once you have configured the build artifacts with `fx set`, you can then build
Fuchsia:

* {fx build}

  ```posix-terminal
  fx build
  ```

  This is what gets run under the hood by `fx build`.

## Rebuilding

In order to rebuild the tree after modifying source code, you can just re-run
`fx build`. This also applies if you modify `BUILD.gn` files as GN adds
Ninja targets to update Ninja targets if build files are changed. The same
holds true for other files used to configure the build.

## Tips and tricks

These tips and tricks only apply for using the `fx gn` command.

### Inspecting the content of a GN target

```posix-terminal
fx gn desc $(fx get-build-dir) //path/to/my:target
```

### Finding references to a GN target

```posix-terminal
fx gn refs $(fx get-build-dir) //path/to/my:target
```

### Referencing targets for the build host

Various host tools (some used in the build itself) need to be built along with
the final image.

To reference a build target for the host toolchain from within a `BUILD.gn`
file:

```
//path/to/target($host_toolchain)
```

### Building only a specific target

If a target is defined in a GN build file as `//foo/bar/blah:dash`, that target
(and its dependencies) can be built with:

Note: This only works for targets in the default toolchain. Building package
targets does not result in an updated package repository, because the package
repository is updated by the `updates` group target. In order for updated
package changes to be made available through `fx serve`, users must build the
`updates` group.

* {fx build}

  ```posix-terminal
  fx build //foo/bar/blah:dash
  ```

* {fx build --host}

  ```posix-terminal
  fx build --host //foo/bar/blah:dash
  ```

### Debugging build timing issues

When running a build, Ninja keeps logs that can be used to view the steps of
the build process. To analyze the timing of a specific build iteration:

1. Run a build as you normally would do. This generates the `.nina_log` file
   in your output directory.

1. Use the `fx ninjatrace2json` tool to convert the Ninja log into a trace file.
   For example:

   ```
   fx ninjatrace2json <your_output_directory>/.ninja_log > trace.json
   ```

1. Load the resulting `trace.json` file in a compatible trace viewer. For
   example, in Chrome, navigate to `chrome://tracing` and click "Load".

Alternatively, you can use `fx report-last-build`. This command gathers
comprehensive build logs and timing information.

[Full comparison of build optimization flags]: #full-comparison-of-build-optimization-flags
[gn-main]: https://gn.googlesource.com/gn/
[gn-preso]: https://docs.google.com/presentation/d/15Zwb53JcncHfEwHpnG_PoIbbzQ3GQi_cpujYwbpcbZo/
[ninja-main]: https://ninja-build.org/
[gn-reference]: https://gn.googlesource.com/gn/+/HEAD/docs/reference.md
[build-project]: /build/
[zircon-getting-started]: /docs/zircon/getting_started.md
