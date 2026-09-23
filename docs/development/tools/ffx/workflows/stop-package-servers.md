# Stop running package servers

The [`ffx repository server`][ffx-repo-server] commands can identify and stop
[Fuchsia package servers][fuchsia-package-server] on the host machine.

## Concepts

The package repository server is controlled by running the subcommands of
[`ffx repository server`][ffx-repo-server]. This server handles requests for
metadata about the available packages and delivers the file blobs that make
up the package contents.

There can be multiple package servers running on a single host machine. It is
useful to be able to stop one or all of these running servers.

There are two ways to stop running package servers on a host machine. You can
stop running a specific server by specifying the name, or stop all running
package servers with --all.

## Basic command

```posix-terminal
ffx repository server stop
```

## Options

The options to `ffx repository server stop` specify which server instances are
stopped.

* No options - Running with no options will stop the running package server
  _if there is only one running server_. Otherwise, an error is returned.

* Positional arguments - Specific servers are stopped by specifying their name
  on the command line as positional arguments. Only servers that have matching
  names are stopped.

* `--all` option - This option stops all running servers.

## Examples

### Stop the one running server

```posix-terminal
ffx repository server stop
```

### Stop a specific server

Stop the server named `workstation_bundle`:

```posix-terminal
ffx repository server stop workstation_bundle
```

### Stop all servers

```posix-terminal
ffx repository server stop --all
```

<!-- Reference links -->

[ffx-repo-server]: /reference/tools/sdk/ffx.md#ffx_repository_server
[fuchsia-package-server]: /docs/concepts/packages/fuchsia_package_server.md
