# Start package servers

The [`ffx repository server`][ffx-repo-server] commands can start and manage
[Fuchsia package servers][fuchsia-package-server] on the host machine.

## Concepts

Virtually all software running on Fuchsia is collected into Fuchsia packages.
A [Fuchsia package](/docs/concepts/packages/package.md) is a hierarchical
collection of files that provides one or more programs, components, or services
to a Fuchsia system. A Fuchsia package is a term representing a unit of
distribution, though unlike many other package systems, that unit is composed
of parts and is not a single binary BLOB.

Beyond the base packages that comprise the foundation of the Fuchsia platform,
additional packages can be downloaded from a Fuchsia package server. The Fuchsia
package server is an HTTP(S) server managing the Fuchsia packages using TUF (the
update framework). This framework uses cryptographically signed BLOBs to
securely distribute updated packages to a device running Fuchsia.

For developers working with Fuchsia, a package server is provided that
facilitates working with packages that are used as the product foundation and
also overlaying packages compiled locally during the development cycle.

## Start the package server

The package repository server is controlled by running the subcommands of
[`ffx repository server`][ffx-repo-server]. This server handles
 requests for metadata about the available packages and delivers the file blobs
 that make up the package contents.

### Basic command

```posix-terminal
ffx repository server start
```

### Server Options

#### --address

The address that the package server listens on to for requests. The format
of the address is can be either an IPv4 or IPv6 address. For example,
`[::]:8083` or  `127.0.0.1:8083`.  Historically, the preferred port for the
package server is `8083`. However there is an effort to improve flexibility by
making the default port `0` indicating to use a dynamic port. The address of
running repository servers is seen by running `ffx repository server list`.

There are no configuration properties influencing the address option. This
option cannot be configured since the port numbers for multiple servers must
be unique.

The motivation for moving to use a dynamic port is to avoid _port in use_
errors. When running the package server locally with a locally connected device
or emulator, ffx can manage the address of the package server internally.

However, there are cases when a specific network address should be used to serve
packages based on the target device's network connection. For example, when
non-ffx commands are needed for tunneling or firewall configuration, specifying
a port is needed to match configuration in these other tools.

#### --background, --daemon, --foreground

The execution mode of the package server. These options are mutually exclusive;
only 1 of the execution modes can be used at a time.

* `--background` Indicates the package server will be started in the
   background. This is mutually exclusive with `--foreground` and `--daemon`.

* `--daemon`  Indicates the package server will be started as part of the ffx
   daemon. This mode should be considered deprecated and only be used when using
   `--background` or `--foreground` is not acceptable. Until the deprecation and
   removal is complete, `--daemon` is the default mode to ensure backwards
   compatibility.
    Note: If you must use `--daemon`, please [file an issue][ffx-bug] explaining the
    missing functionality so the package server can be improved.

* `--foreground` Indicates the package server will be started in the foreground.
   This is mutually exclusive with `--background` and `--daemon`.

There are no configuration properties that affect the execution mode. The
execution mode of the package server is the source of many, sometimes subtle,
problems with developer workflows.

Developers have preferences, depending on individual circumstances on the UI for
the package server being in the foreground or background. The server behavior is
identical regardless of the processing mode.

These are specific switches vs. an option with enumerated values so insight can
be gathered via command line analytics. Switches appear in the analytics, the
value of an option does not.

Since the default behavior via the SDK is to run a daemon based package server,
that is the default for the time being. The package server is migrating to the
foreground being the default so it is obvious that the package server is
running, and is the most simplistic execution model.

Background is a desirable mode for remote workflows so there can be one remote
terminal window, and with IDEs such as VS Code where the user experience of
switching between multiple terminal windows is not easy.

#### --repo-name

Uniquely identifies the package server by name. If another package server is
running with the same name, the new package server will fail with an error.

The default value is devhost. Primarily for historic reasons.

There is no configuration properties affecting `repo-name`; Server names
must be unique.

In `--daemon` mode, this option is not allowed, since repositories are managed
using other commands such as `ffx repository add-from-pm`.

#### --repo-path

The path to the root directory of the repository.

This directory can be a repository initialized with `ffx repository create`, or
the root directory of a product bundle.

The default value is undefined, but can be configured.

The configuration property `package.repository.path` can be set. This is the
root directory of the repository. It is expected that this value is set
per-project vs. at the user level since each project probably has a different
repository.

It is also beneficial to set this configuration property so other package
repository tools such as `ffx repository publish` use the same repository path
as the package server.

This value is also referred to by the publishing tools, and represents a core
characteristic of a development project environment.

In `--daemon` mode, this option is not allowed,since repositories are managed
using other commands such as `ffx repository add-from-pm`.

#### --trusted-root

Path to the root metadata that was used to sign the repository TUF metadata.

This establishes the root of trust for this repository. If the TUF metadata was
not signed by this root metadata, running this command will result in an error.
The default is to use 1.root.json from the repository.

There are no configuration properties that affect `--trusted-root`.

This option is rarely used by developers.

### Registration options

These are options influencing the server registration on the target devices.
As a developer productivity aide, the package server can register itself on
development devices. The following options affect this registration behavior.

#### --no-device

Disables automatic registration of the server on the target device.

Available only with `--foreground` and `--background`.

This flag is most commonly used when working with multiple devices and multiple
projects and the default behavior is doing the wrong thing. It is also useful
when troubleshooting package resolution.

Eventually to simplify the package serving process, this option will be the
default, or registration removed completely. At this point the automatic
registration workflow would be integrated into other commands.

#### --alias

Identifies a package domain alias for this repository.

This is used when registering the package server on a target device in order to
set up a rewrite rule mapping each `alias` domain to this server. Aliases are
listed for running repositories by running `ffx repository server list`.

This only should be used when there is ambiguity about which package server
should be used to resolve a package.

The default is to not have any alias.

There are no configuration properties affecting `--alias`.

This is commonly used when working on a source code project which produces a
package that must be in the "fuchsia.com" or "chromium.org" domain, but is not
part of the fuchsia source project.

#### --alias-conflict-mode

Defines the resolution mechanism when alias registrations conflict.
Must be either `error-out` or `replace`.

The default behavior is `replace`.

There are no configuration properties affecting `--alias-conflict-mode`.

#### --storage-type

Defines the storage type for packages resolved using this registration.
`persistent` defines that the packages are persisted across reboots. Alternatively,
`ephemeral` defines that the packages are lost when rebooting.

The default is `ephemeral`.

There are no configuration properties affecting `--storage-type`.

There are times when a target device needs to reboot and keep the updated
package rather than reverting to the one contained in the last flash or OTA.
This is especially useful when working with packages that are used during the
startup process.

## Examples

### Start the repository in-tree

See [`fx serve` reference](/reference/tools/fx/cmd/serve) for options. As part
of the implementation, `ffx repository server start` is run.

```posix-terminal
fx serve
```

### Start the repository when using VSCode (single terminal window)

```posix-terminal
ffx repository server start --background
```

or in-tree

```posix-terminal
fx serve --background
```

<!-- Reference links -->

[ffx-repo-server]: /reference/tools/sdk/ffx.md#ffx_repository_server
[ffx-bug]: https://issuetracker.google.com/issues/new?component=1378294&template=1838957
[fuchsia-package-server]: /docs/concepts/packages/fuchsia_package_server.md
