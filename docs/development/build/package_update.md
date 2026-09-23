# Developing with Fuchsia packages

This document explains the basics of Fuchsia product development using
[Fuchsia packages][packages].

## Overview

A [Fuchsia package][fuchsia-packages] is a hierarchical collection of files and
resourcesthat provides one or more programs, components or services to a Fuchsia
system. Whether it is immediately apparent or not, almost everything you see on
Fuchsia lives in a Fuchsia package.

During Fuchsia product development, if you want to make new software available
to your target Fuchsia device, you can build a Fuchsia package to include changes
and publish it to a local package server running on your development host machine.
Then the Fuchsia device will download the package and update the software.

In this setup, your development host machine runs a simple static file HTTP
server that makes Fuchsia packages available to the target device. This HTTP
server (which is referred to as the
[Fuchsia package server][fuchsia-package-server]) is part of the Fuchsia source
code and is built automatically when the `fx build` command is run.

Fuchsia target devices are usually configured to look for changes on the
Fuchsia package server. When the update system on a target device sees
changes, it fetches the new package from the package server. Once the package
is downloaded, the new software in the package becomes available on the target
device.

## Prerequisites

Your development host machine and Fuchsia target device must be able to
communicate over TPC/IP. In particular, it must be possible to establish
an SSH connection from the development host machine to the target device.
This SSH connection is used to issue commands to the target device.

## Building packages

For building a package containing your code in the Fuchsia source checkout,
you need to specify a package build rule. For more details on adding build
rules for a package, see [Fuchsia build system][pkg-doc].

Once an appropriate build rule is added, run the following command to
generate your package:

```posix-terminal
fx build
```

## Inspecting packages

Fuchsia comes with the `ffx` tool to work with package archives. After the
completion of a [Fuchsia build][fuchsia-build] (`fx build`), you can use the
[`ffx package archive`][ffx-package-archive] command to create, list, dump or
extract package archives, for example:

```posix-terminal
ffx package archive list
```

The `ffx package archive list` command example below lists the contents
of a package named `meta.far` in the
`~/fuchsia/out/default/obj/third_party/sbase/sed_pkg/` directory:

```none {:.devsite-disable-click-to-copy}
$ ffx package archive list ~/fuchsia/out/default/obj/third_party/sbase/sed_pkg/meta.far
+-------------------------------+
| NAME                          |
+===============================+
| meta/contents                 |
+-------------------------------+
| meta/fuchsia.abi/abi-revision |
+-------------------------------+
| meta/package                  |
+-------------------------------+
```

Notice that this `meta.far` package contains a file named `meta/contents`.
The `meta/contents` file maps the package's user-facing filenames to
the merkle root of those files, for example:

```none {:.devsite-disable-click-to-copy}
$ ffx package archive cat ~/fuchsia/out/default/obj/third_party/sbase/sed_pkg/meta.far meta/contents
bin/sed=361c6e3a027cf38af0d53e1009ef3b3448f4aeb60270910a116396f5ec8eccba
lib/ld.so.1=c8ec55a14590e7e6f06f311d632744012f85084968d98a335c778f29207caa14
lib/libc++.so.2=d8a45259c74cf65c1f8470ac043c6459403fdd0bbf676092ba82f36451e4e571
lib/libc++abi.so.1=f12b37c2b9bfb43fbe33d446794a6ffeee1452b16b7c8d08b3438f3045d1f19a
lib/libfdio.so=9834d890adedc8e532899a673092271e394458e98e6cee049cb78f5a95372b11
lib/libunwind.so.1=89e5dee07ff8d2a03c58d524d8d832e79c70dcb81508f8a83c6fbf07e1539e83
```

For more details on Fuchsia packages and their contents,
see [Structure of a package][pkg-struct].

## Managing package servers

Virtually all software running on a Fuchsia system is collected into Fuchsia
packages. Beyond the base packages that comprise the foundation of the Fuchsia
platform, additional packages can be downloaded from the
[Fuchsia package server][fuchsia-package-server]. The Fuchsia package server is
an HTTP(S) server managing Fuchsia packages using TUF (The Update Framework).
This framework uses cryptographically signed BLOBs to securely distribute
updated packages to devices running Fuchsia.

For more information on how to start, stop, and list Fuchsia package servers
running on your development host machine, see the following guides:

* [Start package servers][start-package-servers]
* [List running package servers][list-package-servers]
* [Stop running package servers][stop-package-servers]

## Connecting host and target

The [Fuchsia source checkout][fuchsia-source] (`fuchsia.git`) contains the Fuchsia
package server, a simple HTTP server that serves static files. And a Fuchsia build
generates and serves a [TUF][TUF-home] file tree to the server.

The update agent on a target Fuchsia device does not initially know where to
look for updates. To connect the update agent on the target to the package server
running on the development host machine, the target must be told the IP address
of the development host machine.

By default, the Fuchsia package server automatically registers itself with the
default target. This behavior can be disabled by using the `--no-device` option
when starting the server.

The following commands are used for managing package repositories on target
devices:

* [`ffx target repository list`][ffx-target-repo-list]: Inspect registered
  package repositories on a target device.
* [`ffx target repository register`][ffx-target-repo-register]: Register a new
  package repository or update an existing entry.
* [`ffx target repository deregister`][ffx-target-repo-deregister]: Remove a
  package repository registration from a target device.

To start the Fuchsia package server and configure the update agent on target
devices, you can run the following command (or simply `fx serve`):

```posix-terminal
fx serve -v
```

The `fx serve` command runs the package server and is often what developers use.
However, the `-v` option is recommended because it allows the command to print
more output, which can help you with debugging. If the packager server connects
successfully to the target devices, the command prints the
`Ready to push packages!` message in the shell on the development host machine.

The update agent on a target device remains configured until it is re-flashed.
The Fuchsia package server attempts to reconfigure the update agent when the
target device is rebooted.

## Triggering package updates

In Fuchsia, packages are not "installed" but they are cached on an as-needed
basis.

There are two collections of packages in a Fuchsia ecosystem:

* **Base**: The `base` package set is a group of software critical to proper
  system function that must remain congruent. The Fuchsia build system
  assigns packages to `base` when they are provided to the `fx set` command
  using the `--with-base` flag.

  This type of software can only be updated by performing a whole system update,
  typically referred to as an OTA (Over The Air) update, which is performed
  using the `fx ota` command.

* **Ephemeral software**: Packages included in the `cache` or `universe` package
  set are ephemeral software. These updates are delivered on demand. The Fuchsia
  build system assigns packages to `universe` when they are provided to the
  `fx set` command using the `--with` flag.

  Ephemeral software is updated to the latest available versions when new
  packages are fetched to the target.

The following commands are used for publishing Fuchsia packages and updating
target devices:

* [`fx serve`][fx-serve]: Run the Fuchsia package server locally for both
  build-push and OTA to target devices.
* [`fx ota`][fx-ota]: Trigger a full system update and reboot on target
  devices.
* [`fx test`][fx-test]: Build and run tests for testing target devices.

## Triggering an OTA

Sometimes there may be many packages changed. For instance, the kernel may
change or there may be changes in the system package. To introduce kernel
changes or changes in the system package to target devices, an OTA or flashing
is required since `base` packages are immutable for the runtime of a system.
(An OTA update will usually be faster than flashing the device.)

To trigger an OTA update to your target devices, run the following command:

```posix-terminal
fx ota
```

The `fx ota` command asks the target device to perform an update from any of
the update sources available to it. To OTA update a build on the development
host machine to a target device on the same LAN, you first need to build the
system (`fx build`). Once the build is finished, if the `fx serve [-v]`
command isn't already running on your development host machine, run the
command so the target device can use the Fuchsia package server as an update
source. The `-v` option prints more information about the files that the
target device is requesting from the package server. With the `-v` option,
there will be a flurry of output as the target device retrieves all the new
files.

After completing the OTA, the target device will reboot.

## Issues and considerations

Consider the following issues that can occur while working with Fuchsia
packages.

### You can fill up your disk

Every update pushed is stored in the content-addressed file system, `blobfs`.
Following a reboot, the updated packages may not be available because the
index that locates them in `blobfs` is only held in RAM. The system currently
does not [garbage collect][gc] inaccessible or no-longer-used packages (having
garbage to collect is a recent innovation!), but will eventually.

The `fx gc` command will reboot the target device and then evict all old
ephemeral software from the device, freeing up space.

### Restarting without rebooting

If the package being updated hosts a service managed by Fuchsia, that service
may need to be restarted. Rebooting is undesirable because both it is slow and
the package will revert to the version flashed on the device. Typically
a developer can terminate one or more running components on the system, either
by asking the component to terminate gracefully, or by forcefully stopping the
component using the [`ffx component stop`][ffx-component-stop] command. Upon
reconnection to the component services, or by invocation via
`ffx component start` or `fx test`, new versions available in the package server
will be cached before launch.

### Packaging code outside the Fuchsia tree

Packaging and pushing code that lives outside the Fuchsia tree is possible, but
will require more work. The Fuchsia package format is quite simple. It consists
of a metadata file describing the package contents, which is described in more
detail in the [Fuchsia package][pkg-struct] documentation. The metadata file is
added to a TUF file tree and each of the contents are named after their merkle
root hash and put in a directory at the root of the TUF file tree called `blobs`.

<!-- Reference links -->

[packages]: /docs/concepts/packages/README.md
[fuchsia-packages]: /docs/concepts/packages/package.md
[pkg-struct]: /docs/concepts/packages/package.md#structure-of-a-package
[TUF-home]: https://theupdateframework.github.io "TUF Homepage"
[pkg-doc]: /docs/development/build/overview.md
[fuchsia-build]: /docs/get-started/learn/build/build-system.md
[ffx-target-repo-list]: https://fuchsia.dev/reference/tools/sdk/ffx#ffx_target_repository_list
[ffx-target-repo-register]: https://fuchsia.dev/reference/tools/sdk/ffx#ffx_target_repository_register
[ffx-target-repo-deregister]: https://fuchsia.dev/reference/tools/sdk/ffx#ffx_target_repository_deregister
[fuchsia-package-server]: /docs/concepts/packages/fuchsia_package_server.md
[ffx-package-archive]: https://fuchsia.dev/reference/tools/sdk/ffx#ffx_package_archive
[fx-serve]: https://fuchsia.dev/reference/tools/fx/cmd/serve
[fx-ota]: https://fuchsia.dev/reference/tools/fx/cmd/ota
[fx-test]: https://fuchsia.dev/reference/tools/fx/cmd/test
[start-package-servers]: /docs/development/tools/ffx/workflows/start-package-servers.md
[list-package-servers]: /docs/development/tools/ffx/workflows/list-package-servers.md
[stop-package-servers]: /docs/development/tools/ffx/workflows/stop-package-servers.md
[fuchsia-source]: /docs/get-started/get_fuchsia_source.md
[gc]: /docs/concepts/packages/garbage_collection.md
[ffx-component-stop]: /docs/development/tools/ffx/workflows/start-a-component-during-development.md#stop-a-component
