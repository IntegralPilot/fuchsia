# fx workflows

This document covers the essential workflows for using the `fx` tool in
the Fuchsia development environment.

## Setting up fx {:#setting-up-fx}

`fx` is the entry point for a set of subcommands that make many tasks
related to Fuchsia development easier. (Run `fx help` to see all the
available subcommands.)

If you use `bash` or `zsh` as a shell, it is strongly recommended to run
`source scripts/fx-env.sh` to enable auto-completion in your shell.
This setup is tested and regularly used with `bash` and `zsh`, and
it may also work for other compatible shells.

1. Navigate to your Fuchsia checkout, for example:

   ```posix-terminal
   cd ~/Fuchsia
   ```
1. Add a configuration to your shell to include `fx-env.sh`:

   ```posix-terminal
   echo "source \"$PWD/scripts/fx-env.sh\"" >> "$HOME/.$(basename "$SHELL")rc"
   ```

1. (Optional) If you would like additional convenience tools from the Fuchsia
   scripts, run:

   ```posix-terminal
   echo "fx-update-path" >> "$HOME/.$(basename "$SHELL")rc"
   ```

1. Restart your shell:

   ```posix-terminal
   exec "$SHELL"
   ```

These steps provides the most well defined feature set, and should be generally
non-invasive. If it causes bugs in your shell environment, please file project
bugs.

If for some reason you need to work with multiple Fuchsia checkouts
(recommended workflows below should obviate such a need), then you may want
to do something other than the above. In this case, there are a few well
supported methods:

* Always execute `$FUCHSIA_DIR/scripts/fx` explicitly
* Use a tool like [direnv](https://direnv.net/){:.external} or
  [dotenv](https://www.npmjs.com/package/dotenv){:.external} to add
  `$FUCHSIA_DIR/.jiri_root/bin` to your `$PATH` while working in a particular
  Fuchsia directory.

Important: Do not copy `fx` outside of `$FUCHSIA_DIR/scripts/` (such as to
`~/bin/`) since this will prevent future updates of `fx` on your system.
Additionally, do not add `$FUCHSIA_DIR/scripts` to your `$PATH`. Since the
`$FUCHSIA_DIR/scripts` directory contains binaries with generic names (e.g.
`bootstrap`), this may unintentionally override the behavior of other systems.

## Common daily tools {#common-daily-tools}

The first thing you will want to do after checking out a Fuchsia tree is to
build Fuchsia, and then get it onto a device. `fx` has some commands to help
with this:

* `fx set` [configure a build](#configure-a-build)
* `fx build` [execute a build](#execute-a-build)
* `fx mkzedboot` [Prepare a Zedboot USB drive](#prepare-zedboot)
* `fx serve` [serve a build](#serve-a-build)
* `fx publish cache` [iterate on cache packages](#iterate-cache-packages)
* `fx ota` [update a target](#update-a-target-device)
* `fx test` [execute tests](#execute-tests)
* `fx shell` [connect to a target shell](#connect-to-a-target-shell)
* [and many other small tasks](#performing-other-common-tasks)

## Target, host, and end-to-end contexts {#target-host-e2e-contexts}

Fuchsia development involves compiling and running code for different
environments:

1.  **Target**: The Fuchsia device or emulator (e.g., x64 or arm64 Fuchsia).
2.  **Host**: Your development workstation (e.g., Linux or macOS).

### `fx build`

*   **`fx build` (Default)**: Builds artifacts intended to run on the **Fuchsia
    target** (drivers, components, command line tools).
*   **`fx build --host`**: Builds tools or tests intended to run on your
    **development host** (e.g., `fidlc` compiler, `zxdb` debugger, host-side
    unit tests, end_to_end tests).
*   **When to use `--host`**: Use it when you are working on a developer tool, a
    host-side test, or need to build a host binary without building the entire
    Fuchsia system image.

### `fx test`

*   **`fx test` (Default)**: Runs the selected tests. It automatically detects
    whether a test is a host test or a device test and executes it in the
    appropriate environment.
*   **`fx test --host`**: **Filters** the selection to **only run host tests**
    (running on your workstation).
*   **`fx test --device`**: **Filters** the selection to **only run device
    tests** (running on the Fuchsia device/emulator).
*   **`fx test --e2e`**: **Enables running end-to-end tests**. These tests run
    on the host machine but interact with the Fuchsia device to test system-wide
    behaviors. They are typically excluded from default test runs because they
    require a connected device and can be slow or create heavy loads.

## Configure a build {#configure-a-build}

First let's configure the build. To do this you need to make a few choices:

* What [product configuration](#key-product-configurations) do you want?
  (unsure: try `workbench_eng`)
* What board are you building for? (unsure: try `x64`)
* What extra [build targets](#key-bundles) do you want? (e.g.:
  `--with-test //my/team:tests`)

Armed with our above choices (if you didn't read above, do so now), you are
ready to configure your build:

```posix-terminal
fx set workbench_eng.x64 --with-test //my/team:tests
```

Once you ran `fx set` in your checkout, there is no need to run it again unless
you need to modify the arguments that follow.

To add more tests to your build configuration without running `fx set`, you can
use these commands:

*   **`fx add-test <label>`**: Appends a target-side test label (adds to
    `developer_test_labels`).
*   **`fx add-host-test <label>`**: Appends a host-side test label qualified
    for the host toolchain (adds to `host_test_labels`).

`fx set` stores its configuration in an `args.gn` file in the output directory.
The output directly is `out/default`. You can specify a different directory
using `fx --dir <out_dir> set <set_args>`.

You can edit the generated `args.gn` file using the `fx args` command to create
more elaborate configurations. `fx args` will open `args.gn` in an editor, let
you make changes, and regenerate the build graph.

Note: If you edit `args.gn` directly then run `fx gen` to regenerate the build
graph.

### What just happened?

* You selected the product `workbench_eng` (run `fx list-products` for a list
  of other product configurations).
* You selected the board `x64`, which supports many typical boards based on the
  `x64` architecture. (Note that `arm64` boards are less interchangeable, and
  you will most likely need to give `fx set` the specific board when building
  for an `arm64` target hardware. Note also that the generic `arm64` board
  configuration does not support running in the emulator; you must use
  `qemu-arm64` instead. Run `fx list-boards` for a list of known board
  configurations.)
* You are ephemerally building `tests` as part of the `universe` package set.

### Package deployment options

The `--with` option has three variants related to how packages are deployed to a
Fuchsia device: `--with-base`, `--with-cache`, and `--with` (implies
`universe`).

In addition to these deployment axes, `fx set` provides flags for specific
contexts:

*   **`--with-host`**: Adds tools or tests that run on your **development host**
    (e.g., `zxdb`). These are not deployed to the Fuchsia device. See [Host
    Context](#target-host-e2e-contexts).
*   **`--with-test`**: Adds specific **tests** to the build (e.g.,
    `//my/team:tests`). These are served ephemerally on demand.

So what are `base`, `cache` and `universe`?

Configurations ultimately specify dependencies (mostly packages) that
contribute to output artifacts of the build (mostly images and package
repositories). The build is parameterized to determine which dependencies
(mostly packages) are added to which output artifacts (images or package
repositories). The three axes are called "base", "cache", and "universe":

* *Base*: Packages that are added to base are included in system images
  produced by the build. They are included in over-the-air updates, and are
  always updated as a single unit. Packages in base can not be evicted from a
  device at runtime - they encode the minimum possible size of a
  configuration.
* *Cache*: Packages in cache are included in system images, but they are not
  included in over-the-air system updates, and are allowed to be evicted from
  the system in response to resource demands, such as disk-space pressure.
  Packages in cache can be updated at any time that updates are available, and
  each of these packages may be updated independently. This is software that
  is "optional", but is good to have available instantly "out of the box".
* *Universe*: Packages in universe are additional optional packages that can be
  fetched and run on-demand, but are not pre-baked into any system images.

The "board" and "product" configurations pick a predefined set of members for
each of these package sets. Most commonly the board configurations specify a
set of boot-critical drivers to add to the base dependency set, and could
for example include some optional but common peripheral drivers in the
cache set. The board configuration may also include some board-specific
development tools (more commonly host tools, rather than target packages) for
interacting with the board in "universe". The product configurations make
choices to add more or less software to the base, cache or universe
package sets based on the definition and feature set of the product they
represent. A speaker product, for example, adds many audio-media-related
packages to the base. A workbench product adds a wide range of GUI,
media and many other packages to the base.

### Key product configurations {#key-product-configurations}

There are many more than below, but the following three particularly
important configurations to be familiar with:

* `bringup` is a minimal feature set product that is focused on being very
  simple and very lean. It exists to provide fast builds and small images
  (primarily used in a [netboot](#what-is-netbooting) rather than
  fashion), and is great for working on very low-level facilities, such as
  the Zircon kernel or board-specific drivers and configurations. It lacks
  most network capabilities, and therefore is not able to add new software
  at runtime or upgrade itself. This also means some `fx` commands such as
  <code>[fx serve](#serve-a-build)</code> and
  <code>[fx shell](#connect-to-a-target-shell)</code> cannot be used with
  the `bringup` product.
* `core` is a minimal feature set that can install additional software (such as
  items added to the "universe" dependency set). It is the starting point for
  all higher-level product configurations. It has common network capabilities
  and can update a system over-the-air.
* `workbench_eng` is a basis for a general purpose development environment, good
  for working on UI, media and many other high-level features. This is also
  the best environment for enthusiasts to play with and explore.

### Key additional build targets {#key-bundles}

The `--with` and `--with-test` flags for `fx set` take in arbitrary
[build targets](/docs/development/build/overview.md#build-targets).
For convenience, a number of bundles are defined, which include a variety of
commonly used build targets. It is important to be familiarized with the
following bundles:

* `//bundles/tools` contains a broad array of the most common developer tools.
  This includes tools for spawning components from command-line shells, tools
  for reconfiguring and testing networks, making http requests, debugging
  programs, changing audio volume, and so on. The core product includes
  `//bundles/tools` in the universe package set by default.
* `//bundles/tests` causes all test programs to be built. Most test programs
  can be invoked using `run-test-suite` on the device, or via
  `fx test`.
* `//bundles/kitchen_sink` is a target that causes all other build targets to be
  included. It is useful when testing the impact of core changes, or when
  making large scale changes in the code base. It also may be a fun
  configuration for enthusiasts to play with, as it includes all software
  available in the source tree. Note that kitchen sink will produce more than
  20GB of build artifacts and requires at least 2GB of storage on the target
  device (size estimates from Q1/2019).

## Execute a build {#execute-a-build}

Once you configured your build with `fx set` as shown above, you can run a build
with `fx build`. This commands builds all required targets and their outputs.

### Clean a build

You can continue to make changes to files in your checkout and run `fx build` to
rebuild. The build system will attempt to do less work when there are results from
a previous build. A build that uses previous build results is called an incremental
build, and is usually much faster than a clean rebuild.

Changing configurations should not result in a broken incremental build, but this
may still happen in rare cases due to [limitations of the build system][fxb94507].
If this happens, please file a detailed bug that captures any steps to reproduce the
problem and any diagnostics such as build logs. Then use these commands to recover:

* `fx clean` will clear out all build artifacts.
* `fx clean-build` is equivalent to `fx clean`, then `fx build`.
* `fx cleandead-build` is equivalent to `fx cleandead`, then `fx build`

### Enabling incremental package rebuilds

By default, `fx build` builds all packages for the specified product configuration.
This is necessary for some developer workflows, but it is excessive for many others.
If you are only iterating on your test, you shouldn't need to rebuild and republish
all ephemeral (universe) packages.

You can enable incremental rebuilds by adding `export FUCHSIA_DISABLED_incremental=0`
to your `~/.bashrc` or equivalent. This change results in the following:

* (`fx serve`)[fx-serve-ref] watches for packages that are created or modified
  in your Fuchsia build and automatically publishes them. This allows you to
  keep `fx serve` running from an empty tree and it will publish changes
  incrementally as you work.

* `fx test` only builds the minimal targets required for running. For component tests,
  that's the package, its GN dependencies and `//zircon`.

Note that the behavior of `fx build` remains unchanged.

### Iterating on cache packages {#iterate-cache-packages}

Running `fx build` triggers a full system assembly in addition to compiling and
publishing [cache packages](#package_deployment_options). If your workflow requires
only iterating over rebuilding cache packages, running `fx build` can be excessive
and cause slowdowns.

For rebuilding cache packages, you can achieve faster build speed by replacing
`fx build` with `fx publish cache`. Below is what it might look like in a typical
development workflow:

```none {:.devsite-disable-click-to-copy}
# Set up your workspace here.
$ fx serve

# Make changes to cache packages here.

# Rebuild and publish cache packages:
$ fx publish cache

# Restart your component, for example:
$ ffx component stop /core/your_component
$ ffx session restart
```

A known issue: Rebooting the Fuchsia emulator does not cause components to run
from the updated packages. The components needs to be restarted manually.

To view a list of all cache packages, you can run `fx list-packages --cache`.

### Building a specific target {#building-a-specific-target}

`fx build` can be given the name of a specific target or file to build.

If the experimental [build-with-labels] feature is enabled,
one can pass the GN label directly to `fx build`. For example
`fx build //examples/hello_world` will build the `//examples/hello_world:hello_world`
GN target for Fuchsia, and `fx build --host //examples/hello_world` will build
it for the host machine.

If the feature is not enable, you can also pass a Ninja target path for one of your
GN target's output. For GN targets defined in the default toolchain, GN defines Ninja
aliases that look like the GN label without a starting `//` prefix, thus
`fx build examples/hello_world:hello_world` will build the outputs of the GN
target `//examples/hello_world` in the default toolchain.

If the targets are declared in a non-default toolchain though, you will have to
guess the Ninja output path, which is generally very tricky. For host binaries
though, it is typically under the `host_x64` sub-directory. Hence
`//foo/bar:blah(//build/toolchain:host_x64)` can be built with `fx build host_x64/blah`
if that target is defined as an `executable()` only.

See the [build system overview][build-overview] for a more detailed discussion
of build targets.

### Creating a product bundle ZIP file {#creating-a-product-bundle-zip-file}

If you've already built a Fuchsia product (by running `fx build`), you can
run the following command to [create a ZIP file][fx-create-pb-zip] that
contains the product bundle:

```posix-terminal
fx create-pb-zip -o <path-to-pb-zip>
```

This command creates a zip file containing the product bundle in the
path you specified.

### Building and uploading a product bundle ZIP file {#building-and-uploading-a-product-bundle-zip-file}

The `fx build-and-upload` command provides a convenient way to build Fuchsia,
create a product bundle ZIP file, and upload it to a Google Cloud Storage (GCS)
bucket in a single step.

```posix-terminal
fx build-and-upload
```

By default, this command uploads the product bundle to the
`gs://fuchsia-build-stage` bucket. However, you can specify a different GCS
bucket using the `--bucket` (or `-b`) flag:

```posix-terminal
fx build-and-upload --bucket gs://<bucket-name>
```

Once the upload is complete, the printed GS URL can be used directly with `ffx`
commands for flashing a target device or starting the emulator, for example:

```none {:.devsite-disable-click-to-copy}
# Flash a target device
$ ffx target flash -b gs://fuchsia-build-stage/pb.zip
```

```none {:.devsite-disable-click-to-copy}
# Start the emulator
$ ffx emu start gs://fuchsia-build-stage/pb.zip -H
```

## Prepare a Zedboot USB drive {#prepare-zedboot}

The exact preparation required to flash Fuchsia onto a target device varies by
specific device, but you can use `fx mkzedboot` prepare a bootable USB driven
for `x64` devices that boots into Zedboot.

### What is Zedboot? {#what-is-zedboot}

Zedboot is a special configuration of Zircon that contains a simple network
stack, a simple device advertisement and discovery protocols, and a suite of
protocols to write Fuchsia to a target disk and/or to network boot a target
system. Zedboot is a term used for both the overall process, as well as a
special build configuration. Many people come to know it as "the blue screen
with ASCII art".

To enter Zedboot on an arm64 target, power on the device while triggering a
boot into fastboot flashing mode (often this involves holding a particular
button while rebooting or powering on that varies by particular hardware
target). Once in flashing mode, execute `fx flash` on the host system.

To enter Zedboot on an x64 target, first produce a Zedboot USB key using
`fx mkzedboot <path-to-usb-device>` (to list suitable USB devices on your
system, execute `fx list-usb-disks`). Remove the USB key after completion,
insert it to the target device, and reboot the target device, selecting "Boot
from USB" from the boot options, or in the device BIOS.

### What is Netbooting? {#what-is-netbooting}

In Fuchsia, "netboot" refers to sending a set of artifacts to a Zedboot
instance that instead of making changes to the disk, will just be booted from
RAM. Users can perform a "netboot" by first booting a device into Zedboot by
using either `fx flash` (arm64) or `fx mkzedboot` (x64), and then executing
`fx netboot` on the host system.

Note: the `netboot` artifacts are not produced by all builds by default,
because for larger builds such as the "workbench" product configuration
such builds are extremely large, and producing them many times a day is both
slow as well as measurably wearing on host disk hardware. The `bringup`
configuration always prepares `netboot` artifacts. For all other build
configurations, a user can optionally build the netboot artifacts using
`fx build netboot`.

## Serve a build {#serve-a-build}

A lot of build configurations for Fuchsia include software that is not
immediately included in the base images that a build produces, that are
written to devices during flashing. Such software is instead made available to
target devices on-demand, which is often colloquially referred to as
"ephemeral software".

The command `fx serve` performs two functions internally:

* Start a package repository server, used for dynamic
  installation of software at runtime, as well as whole-system updates.

Internally the [`fx serve`][fx-serve-ref] command also searches for a device to
configure, and upon discovery (which may be restricted/modulated with
`fx set-device` or `fx -t`) the target device is configured to use the
repository server as a source of dynamic packages and system updates.

## Update a target device {#update-a-target-device}

As described in prior sections, there are different groups of software on a
Fuchsia device:

* Software that is part of the core system "base", that is updated in a
  single transaction.
* Software that is part of Zedboot images other than base (cache)
  that can be updated ephemerally.
* Software that is always ephemeral (universe).

For new user development workflows, the most general command to assist with
updating a target device is `fx ota`. The `fx ota` command first
updates "base" and "cache" software, and then reboots the target device
when it is complete. The end result of this process should be
indistinguishable in terms of software versions from performing a fresh
flash of a device.

As the `fx ota` process causes a device reboot, it is sometimes not the
most efficient process for diagnosis, debugging or other non-testing based
workflows or needs. In these cases a user has some options for how to ensure
that software on a device is being regularly updated.

The `fx serve` process configures a Fuchsia software repository with
automatic update features. The repository informs the target device of newly
updated software every time the underlying repository is updated (which
happens at the end of every successful `fx build`). For many software
components, the easiest way to update them during development is to ensure
that they are not included in the base set, but instead included in
either "cache" or "universe". In that case, simply restarting the
software on the target (e.g. by closing it completely, or by invoking
`killall`) will result in the software being immediately updated when it is
started again.

Note: some software may not appear to be updating because it is being run
inside of a "runner" process or some other surrounding environment that is
"holding on" to resources for the previous package version, only spawning
programs from the old package. As packages in Fuchsia are immutable and
content-addressed, when host environments retain resources in this manner,
there is nothing that the update system can do to forcefully trigger updates
in the rest of the system. Users who find themselves with this issue mostly
need to find efficient workflow methods to fully restart the relevant
software stack.

## Execute tests {#execute-tests}

The Fuchsia codebase contains many tests. Most of these tests are themselves
components and can be launched on the target device in the same way as other
components. On the target device, some programs also assist with test-specific
concerns for component launching, such as `run-test-suite`. The process can also
conveniently be controlled from the development host by way of `fx test`. See
[Run Fuchsia tests][executing-tests] for more details.

Some users find that an effective high focus workflow is to have the system
build, push and execute tests whenever they save their source code. This can
be achieved with `fx` very easily, for example:

```posix-terminal
fx -i test hello-world-rust-tests
```

The above command will execute the tests every time a change is made
to the source code in the tree. The `-i` flag to `fx` causes `fx` to repeat
the rest of its command every time the source code in the tree is changed.
As the `fx test` command first performs a build, then executes a test on
a target, this combination provides a convenient auto-test loop, great for
high focus workflows like test driven development.

Note: Iterative mode (indicated by the *-i* option) requires the `inotify-tools`
or `fswatch` package on the host system.

## Connect to a target shell {#connect-to-a-target-shell}

Most [product configurations](#key-product-configurations) include an SSH
server with a Fuchsia specific configuration. The command `fx shell` is a
convenient wrapper to connect to the target device over SSH and provides
access to a very simply POSIX-style shell. Users should note that while the
shell is a fork of a POSIX shell, it does not provide all features of a
common Unix shell. In particular users will find that CTRL+C has odd quirks,
and may often find quirks for sub-shell expressions and certain more advanced
IO redirections or environment variable propagations. These misfeatures are
side effects of Fuchsia not being a POSIX system.

Nonetheless the shell made available via `fx shell` is extremely useful for
imperatively executing programs on the Fuchsia target, as well as exploring
some of the diagnostic / debug interfaces made available in a filesystem
tree, such as `/hub` and `/dev`. It is also useful for invoking programs such
as `/bin/run` that provides facilities for launching Fuchsia components. If
the `tools` bundle is available in the build configuration, many tools common
to unix shell environments have been ported and are available, such as `ps`,
`ls`, `cat`, `curl`, `vim`, `fortune` and so on.

## Performing other common tasks {#performing-other-common-tasks}

### Getting logs {#getting-logs}

`fx log` captures all logs from low-level and high-level programs,
including the kernel, drivers and other userspace programs. `fx log`
depends upon a working high level network stack and SSH. As such, `fx log`
does not work with Zedboot or "bringup" product configurations. If a device
is in a state where `fx log` ceases to function, it is often useful to
switch to `fx klog` to capture more information about probable causes.

`fx klog` captures only a low-level log stream called "klog". The klog stream
includes logs from the Zircon kernel itself, as well as a subset of userspace
software (most notably drivers and low-level core software). `fx klog` depends
on a lightweight network stack called `netsvc` that has a tendency to remain
available even after problems in higher-level software. The netsvc suite is
also always available in "bringup" product configurations, as such, `fx klog`
is most useful when working on low-level software, such as the Zircon kernel,
or drivers.

See [Viewing Logs](/docs/development/diagnostics/logs/viewing.md) for more information.

### Copying files {#copying-files}

`fx cp` provides a basic wrapper around `scp`, similar to how `fx shell` is a
wrapper around `ssh`.

```none {:.devsite-disable-click-to-copy}
# copy ./book.txt from the host, to /tmp/book.txt on the target
$ fx cp book.txt /tmp/book.txt
# copy /tmp/poem.txt on the target to poem.txt on the host
$ fx cp --to-host /tmp/poem.txt poem.txt
```

### Using multiple Fuchsia devices {#using-multiple-fuchsia-devices}

Some users will have more than one Fuchsia device on a network, and will want
to limit the effects of various commands to particular of those devices. The
`fx set-device` command exists to help with this use case.

The `fx set-device` command binds a particular device node name to a
particular build directory. This is particularly useful when a user wishes to
keep several different devices in several build configurations, and could be
setup as follows:

```none {:.devsite-disable-click-to-copy}
$ fx --dir out/workbench set workbench_eng.x64
$ fx build
$ fx set-device <workbench-node-name>

$ fx --dir out/core set core.qemu-arm64
$ fx build
$ fx set-device <core-node-name>

# Start a server for the workbench:
$ fx --dir=out/workbench serve
# Set the default build-dir and target device to the arm64 core, and
# connect to a shell on that device:
$ fx use out/core
$ fx shell
```

Additionally, for users who wish to execute a command against a single
Fuchsia device from the current default build directory, as a one-off
command, the `fx` global flag `-d` allows overriding the target node name for
a single command invocation.

### Reboot a device {#reboot-a-device}

`fx reboot`

On some devices (most arm64 devices at present) there are also some useful flags:

* `fx reboot -r` reboot into "recovery" (Zedboot)
* `fx reboot -b` reboot into "bootloader" (Flash)

### Determine a CL's status {#determine-a-cls-status}

You can use the `fx cl` command to open your current CL in Gerrit using a new
browser tab.

### Debugging and developing `fx` commands {#debugging-and-developing-fx-commands}

* `fx -x` the `-x` flag turns on tracing for the `fx` scripts, printing out all
  expressions evaluated during the `fx` invocation.
* `fx exec` executes an arbitrary program that follows inside of the current
  `fx` environment. As an example `fx exec env` prints all environment
  variables in that environment (`fx exec env | grep FUCHSIA` is likely of
  interest).

### Getting help with `fx` {#getting-help-with-fx}

`fx help <command>` provides the best introductory documentation for that
command. Some commands also support and provide `fx <command> -h` or
`fx <command> --help`, however this help is not available for all commands.
This is unusual, but is a function of implementation details. Internally many
`fx` commands just run other programs, most often those produced by the
build, and flags are in many cases passed on unaltered to those programs. In
those cases, passing the usual `-h` or `--help` flags may not provide
documentation for `fx <command>`, but instead for the program invoked
downstream of `fx`.

Users should always start with `fx help <command>`.

`fx help` with no other arguments provides a list of all available commands
in `fx`, as well as documentation for `fx` global flags.

### Displaying pending commits {#displaying-pending-commits}

`fx pending-commits` displays the commits not yet rolled to global
  integration.

To view Fuchsia's integration dashboard, see [Builders](https://luci-milo.appspot.com/p/fuchsia).

### Synchronizing the Fuchsia checkout to a release branch {#synchronizing-to-a-release-branch}

To synchronize your local Fuchsia checkout to a specific release branch,
run the following command:

```posix-terminal
fx sync-to {{ "<var>" }}RELEASE_BRANCH{{ "</var>" }}
```

Replace `RELEASE_BRANCH` with the release branch that you want to switch to
(for example, `refs/heads/releases/f6`). To see the list of all available
release branches, see the Branches section on the
[Fuchsia global integration repository][fuchsia-gi-repo]{:.external} page.

The example command below synchronizes the Fuchsia checkout to the F6 release
branch:

```none {:.devsite-disable-click-to-copy}
fx sync-to refs/heads/releases/f6
```

To reset the Fuchsia checkout to the top of the tree, run the following command:

```posix-terminal
fx sync-to reset
```

For more options, see the [`fx sync-to` reference][fx-sync-to-ref] page.

### Defining persistent local build arguments

If you want to define build arguments that will be included whenever running
`fx set`, add them to `$FUCHSIA_DIR/local/args.gn`. They will be appended to
`$FUCHSIA_BUILD_DIR/args.gn` whenever regenerating the build arguments.

To suppress the inclusion of `local/args.gn`, run `fx set ... --skip-local-args`.

<!-- Reference links -->

[build-overview]: /docs/development/build/overview.md
[build-with-labels]: /docs/development/build/build_with_labels.md
[executing-tests]: /docs/development/testing/run_fuchsia_tests.md
[ffx-target-flash]: /reference/tools/sdk/ffx.md#flash
[fxb94507]: https://bugs.fuchsia.dev/p/fuchsia/issues/detail?id=94507
[fuchsia-gi-repo]: https://fuchsia.googlesource.com/integration/
[fx-sync-to-ref]: /reference/tools/fx/cmd/sync-to.md
[fx-serve-ref]: /reference/tools/fx/cmd/serve.md
[fx-create-pb-zip]: https://fuchsia.dev/reference/tools/fx/cmd/create-pb-zip
