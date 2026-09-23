# List running package servers

The [`ffx repository server`][ffx-repo-server] commands can identify and list
[Fuchsia package servers][fuchsia-package-server] running on the host machine.

## Concepts

The package repository server is controlled by running the subcommands of
[`ffx repository server`][ffx-repo-server]. This server handles requests for
metadata about the available packages and delivers the file blobs that make up
the package contents.

There can be multiple package servers running on a single host machine. It is
useful to be able to list these running servers and their properties. These
properties are useful for troubleshooting or integrating package servers into a
higher level system or workflow automation.

## Basic command

```posix-terminal
ffx repository server list
```

This command supports the [top level `ffx` options](/reference/tools/sdk/ffx) to
support programmatic use of the command:

* `--machine` produces machine reader friendly output
* `--schema` produces the JSON schema of the machine output

## Options

### --full

Produce the full details for each running package server.

### --name

Limit the output to package servers with the given name.
This option can be specified multiple times.

## Output

The default output is a list of:

* name
* listening address
* repo_path

For example:

```none {:.devsite-disable-click-to-copy}
devhost      \[::\]:8083   /path/to/product_bundles/core.x64/repository
```

Full output adds additional fields:

* execution mode
* registration_aliases
* registration_storage_type
* registration_alias_conflict_mode
* pid

## Examples

### List all running servers

```posix-terminal
ffx repository server list
```

### List full details of all running servers

```posix-terminal
ffx repository server list --full
```

### List all details for the server

List servers named `devhost` or `devhost2` and print the output in JSON format:

```posix-terminal
ffx --machine json-pretty repository server list --name devhost --name devhost2
```
<!-- Reference links -->

[ffx-repo-server]: /reference/tools/sdk/ffx.md#ffx_repository_server
[fuchsia-package-server]: /docs/concepts/packages/fuchsia_package_server.md
