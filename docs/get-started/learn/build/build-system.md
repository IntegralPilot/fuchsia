# Build system

Fuchsia builds use [Generate Ninja](https://gn.googlesource.com/gn/) (GN),
a meta-build system that generates build files consumed by
[Ninja](https://ninja-build.org/){:.external}, which executes the actual build.
The build system provides the tools to configure the build for a specific
product and templates to build code for Fuchsia targets.

## Build targets

You define individual build targets for GN using `BUILD.gn` files located with
your project source code. The Fuchsia build system provides templates as GN
imports (`.gni`) for you to declare Fuchsia artifacts, such as:

* `fuchsia_component()`: Defines an executable
  [component](/docs/concepts/components/v2), containing the manifest, program
  binary, and resources.
* `fuchsia_package()`: Defines a [package](/docs/concepts/packages/package.md)
  containing one or more components for distribution in a package repository.
* `fuchsia_test_package()`: Defines a package containing test components.

Note: You can see all the Fuchsia build templates in
[`//build/components.gni`](/build/components.gni).

Below is an example of a `BUILD.gn` file for a simple component package with
tests:

```gn
import("//build/components.gni")

executable("bin") {
  sources = [ "main.cc" ]
}

fuchsia_component("hello-world-component") {
  deps = [ ":bin" ]
  manifest = "meta/hello-world.cml"
}

fuchsia_package("hello-world") {
  deps = [
    ":hello-world-component",
  ]
}

fuchsia_component("hello-world-test-component") {
  testonly = true
  deps = [ ":bin_test" ]
  manifest = "meta/hello-world-bin-test.cml"
}

fuchsia_test_package("hello-world-tests") {
  test_components = [ ":hello-world-test-component" ]
}
```

A unique **label** composed of the target's name and the path to its `BUILD.gn`
file identifies everything that can participate in the build. In the above
example, the `hello-world` target might have a label that looks like
this: `//src/examples/basic:hello-world`.

Note: For more details on the mechanics of building with GN, see
[Introduction to GN](/docs/development/build/gn_concepts/intro.md).

## Build configuration

The GN front-end configures the build according to the chosen Fuchsia
**product configuration**, collecting all the necessary packages and components
required by the build. These targets are defined in various `BUILD.gn` files
throughout the source tree. The output of the GN step is an optimized set of
instructions for Ninja in the build directory.

The build system invokes GN when you run the `fx set` command to configure
the build.

```posix-terminal
fx set minimal.x64
```

<aside class="key-point">
You can also invoke GN directly with <code>fx gn</code> to customize or
troubleshoot the build.
</aside>

You should run the GN configuration step anytime you want to adjust the product
configuration or the packages available to the build. GN is also invoked
automatically during a build anytime one of the `BUILD.gn` files in the current
configuration is changed.

## Boards and products

The Fuchsia build system defines the baseline configuration for a Fuchsia build
as a combination of a **product** and **board**. Together, these elements form
the build configuration you provide to `fx set`.

Boards define the architecture that the build targets, which may affect what
drivers are included and influence device specific kernel parameters.

This codelab targets the `x64` board, which supports the Fuchsia emulator
(FEMU) running on x64 architecture.

<aside class="key-point">
You can discover all the available target boards with
<code>fx list-boards</code>.
</aside>

A product defines the software configuration that a build produces. This
configuration may include what services are available and the user-facing
experience.

This codelab targets the `minimal_eng` product.

<aside class="key-point">
You can discover all the available target products with
<code>fx list-products</code>.
</aside>

## Build

Once the GN build configuration is complete, Ninja consumes the generated build
files and runs the appropriate compile, link, and packaging commands to generate
the Fuchsia image.

The build system invokes Ninja when you run the `fx build` command to execute
the current build configuration.

```posix-terminal
fx build
```

<aside class="key-point">
You can also invoke Ninja directly with <code>fx ninja</code> to customize or
troubleshoot the build.
</aside>

## Exercise: Build Minimal

In this exercise, you'll build the `minimal_eng` product configuration from
source to run on the `x64` emulator board.

### Configure the build

Set up the build environment for the `minimal` product and `x64` board:

```posix-terminal
fx set minimal.x64
```

This command runs GN on the set of targets defined in the product's build
configuration to produce the build instructions. **It does not actually
perform the build**, but instead defines the parameters of what is considered
buildable.


<aside class="key-point">
You can also ask GN to regenerate the existing build configuration using
<code>fx gen</code> without needing to supply the product configuration again.
This can be helpful when you are editing <code>BUILD.gn</code> files and want to
quickly validate without running a full build.
</aside>

### Inspect the build configuration

Once the build is configured, use `fx list-packages` to print the set of
packages the build is aware of:

```posix-terminal
fx list-packages
```

This is a useful tool to determine if a package you need was properly included
in the build configuration.

### Build Fuchsia Minimal

Build the Minimal target with `fx build`:

<aside class="caution">
A full build on a fresh checkout of the source can take upwards of 60-90 minutes
to complete, depending on the capabilities of the build machine. Subsequent
incremental builds will only take a few minutes.
</aside>

```posix-terminal
fx build
```

<<../_common/_restart_femu.md>>

### Inspect the device

Open another terminal window and run the following command to print the details
of your device target:

Note: See the full [`ffx` reference documentation](/reference/tools/sdk/ffx.md).

```posix-terminal
ffx target show
```

Look for the build configuration of the target output:

```none {:.devsite-disable-click-to-copy}
{{ '<strong>' }}Version: "2000-01-01T12:00:00+00:00"{{ '</strong>' }}
Product: "minimal_eng"
Board: "x64"
{{ '<strong>' }}Commit: "2000-01-01T12:00:00+00:00"{{ '</strong>' }}
```

Notice that the configuration points to the build you just completed on your
machine.

You are now running your own build of Fuchsia!

## Exercise: Run a repository server and serve packages

In this exercise, you'll run a repository server to serve the packages available
in the `universe` set to your running Fuchsia target on the emulator.

To learn more about repository and the different package sets, refer
to [RFC-0212 Package Sets](/docs/contribute/governance/rfcs/0212_package_sets.md)

For example, running `ffx debug connect` will fail because the target needs the
a component that exposes the `fuchsia.debugger.Launcher` capability. This is
available in the `debug_agent` package in the `universe` set of `minimal.x64`.

### List the packages in the universe set

Run `fx list-packages` to list the packages in the `universe` set. For
`minimal.x64` there is only one package:

```posix-terminal
fx list-packages --universe
```

The output shows only `debug_agent` package. The `debug_agent` includes a
component that provides the `fuchsia.debugger.Launcher` capability. We will
exercise this capability by launching a debugger in the next steps.

### Start a repository server and register it with the Fuchsia target

The `fx serve` command starts the repository server and registers it with
the target.

```posix-terminal
$ fx serve
```

You will see the following output if the command is run successfully:

```posix-terminal
2024-05-20 12:51:03 [serve] Discovery...
2024-05-20 12:51:07 [serve] Device up
2024-05-20 12:51:07 [serve] Registering devhost as update source
Serving repository '/usr/local/google/home/amituttam/fuchsia/out/minimal.x64/amber-files' to target 'fuchsia-emulator' over address '[::]:8083'.
```

Leave this command running the foreground. To run in verbose mode pass the
`--verbose` or `-v` flag to `fx serve`.

## Exercise: Run the debugger and list the running processes

With the repository server running, and the Fuchsia target configured to resolve
packages from that repository, we can run a command that resolves the package
and runs the component within.

First, let's run logs so we can see what is happening:

```posix-terminal
ffx log --filter debug_agent
```

Run the above command in a separate terminal, the output will be empty. Leave
the command running. Here, we are filtering on `debug_agent` as the system will
need to resolve that package to run the debugger.

In the other terminal, connect the debugger to the running system:

```posix-terminal
ffx debug connect
```

The command should launch `zxdb` and place you in an interactive shell. If you
switch back to the other terminal with the running `ffx log`, you will see
output along the lines of:

```
[01949.544917][pkg-resolver] INFO: attempting to resolve fuchsia-pkg://fuchsia.com/debug_agent as fuchsia-pkg://devhost/debug_agent with TUF
[01949.624075][pkg-resolver] INFO: updated local TUF metadata for "fuchsia-pkg://devhost" to version RepoVersions { root: 9, timestamp: Some(1715399460), snapshot: Some(1715399460), targets: Some(1715399460) } while getting merkle for TargetPath("debug_agent/0")
[01949.835760][pkg-resolver] INFO: resolved fuchsia-pkg://fuchsia.com/debug_agent as fuchsia-pkg://devhost/debug_agent to 3f9783abed30d70b72d5f0730bd6e6033481073126aac0b74cbbf2d14909497e with TUF
[01949.882891][debugger] INFO: [main_launcher.cc(182)] Start listening on FIDL fuchsia::debugger::Launcher.
```

Here the system attempts to resolve the `debug_agent` package and is able to do
so from the configured `devhost` repository. Once it resolves the package, the
component is launched that listens for the debugger launcher protocol.

Back to `zxdb`, you can now run `ps` to see the list of running processes on
the system.

```
[zxdb] ps
 j: 1033 root
   p: 1102 bin/component_manager
   j: 1649
     j: 1780 bootstrap/console fuchsia-boot:///console#meta/console.cm
       p: 1822 console.cm
     j: 1989 bootstrap/archivist fuchsia-boot:///archivist#meta/archivist.cm
       p: 2051 archivist.cm
     j: 2064 bootstrap/console-launcher fuchsia-boot:///console-launcher#meta/console-launcher.cm
...
```