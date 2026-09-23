# Explore components

The `ffx component explore` command starts an interactive shell that lets you explore
the internals of Fuchsia components running on a target device.

## Concepts

The `ffx component explore` command launches a [Dash][dash]{:.external}  process scoped
to the target component. Using this Dash process, you can:

+   Use familiar POSIX commands such as `ls`, `cat` and `grep`.
+   Explore the component's incoming and outgoing capabilities.

Dash is the command interpreter previously used in other tools in the Fuchsia project,
such as `fx shell`, serial console, terminal windows, and `virtcon`. Dash provides
`ffx component explore` a familiar experience to developers, such as `cd` and `ls`, for
navigating spaces in a Fuchsia component, for example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist
Moniker: /bootstrap/archivist
$ {{ '<strong>' }}ls{{ '</strong>' }}
exposed
ns
out
runtime
svc
$
```

Note: The filesystem used in Dash is not POSIX-compliant. Capabilities are
presented as files and directories to aid in navigation.

However, unlike [`fx shell`][fx-shell], the namespace only contains the component's incoming
and outgoing capabilities. This restriction means you can explore in an environment almost
identical to what the component sees.

## Explore a component {:#explore-a-component}

To connect to a Fuchsia component and start an interactive shell, run the following command:

```posix-terminal
ffx component explore {{ '<var>' }}COMPONENT_IDENTIFIER{{ '</var>' }}
```

Replace <var>COMPONENT_IDENTIFIER</var> with the [moniker][component-moniker],
[URL][component-url], or [instance ID][instance-id] of your target component. The command
also accepts a unique partial match on these identifiers.

The example command below starts an interactive shell (`$`) for exploring the
`/bootstrap/archivst` component:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist
Moniker: /bootstrap/archivist
$
```

In all examples in this guide, the shell prompt from the host machine's terminal is
represented as `[host]$` while the prompt from the component's interactive shell is
represented as `$` alone, without the `[host]` prefix. To put it differently,
`[host]$` means that the command is run on the host machine's terminal while `$`
means the command is run on the interactive shell connected to the target component.

This guide also uses `/bootstrap/archivist` (which is a moniker) for the target component
in most examples. In practice, this argument should be replaced with the component identifier
of your target component.

## Explore capabilities available to a component {:#explore-capabilities-available-to-a-component}

To explore the capabilities of the target component, navigate to the `/ns` directory
in the component's interactive shell. The `/ns` directory contains the component's
namespace, exactly as the component would see it, for example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist
Moniker: /bootstrap/archivist
$ {{ '<strong>' }}cd ns{{ '</strong>' }}
$ {{ '<strong>' }}ls{{ '</strong>' }}
config
events
pkg
svc
```

If you want the shell's namespace to match the component namespace,
use the `-l` (or `--layout`) flag, for example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist -l namespace
Moniker: /bootstrap/archivist
$ {{ '<strong>' }}ls{{ '</strong>' }}
config
events
pkg
svc
```

For more details on these directories, see
[What is the namespace root in ffx component explore?](#what-is-the-namespace-root-in-ffx-component-explore).

## Explore capabilities exposed by a component {:#explore-capabilities-exposed-by-a-component}

The `/exposed` directory contains the capabilities exposed from your target
component to its parent, for example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist
Moniker: /bootstrap/archivist
$ {{ '<strong>' }}cd exposed{{ '</strong>' }}
$ {{ '<strong>' }}ls{{ '</strong>' }}
diagnostics
fuchsia.diagnostics.ArchiveAccessor
fuchsia.diagnostics.ArchiveAccessor.feedback
fuchsia.diagnostics.ArchiveAccessor.legacy_metrics
fuchsia.diagnostics.ArchiveAccessor.lowpan
fuchsia.diagnostics.LogSettings
fuchsia.logger.Log
fuchsia.logger.LogSink
```

## Explore capabilities served by a component {:#explore-capabilities-served-by-a-component}

If the target component is running on the device, the `/out` directory
contains all the capabilities currently served by the component, for
example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist
Moniker: /bootstrap/archivist
$ {{ '<strong>' }}cd out{{ '</strong>' }}
$ {{ '<strong>' }}ls{{ '</strong>' }}
diagnostics
svc
$ {{ '<strong>' }}cd svc{{ '</strong>' }}
$ {{ '<strong>' }}ls{{ '</strong>' }}
fuchsia.diagnostics.ArchiveAccessor
fuchsia.diagnostics.ArchiveAccessor.feedback
fuchsia.diagnostics.ArchiveAccessor.legacy_metrics
fuchsia.diagnostics.ArchiveAccessor.lowpan
fuchsia.diagnostics.LogSettings
fuchsia.logger.Log
```

## Explore debug runtime data of a component {:#explore-debug-runtime-data-of-a-component}

If the target component is running on the device, the `/runtime` directory
contains debug information provided by the component runner, for example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist
Moniker: /bootstrap/archivist
$ {{ '<strong>' }}cd runtime/elf{{ '</strong>' }}
$ {{ '<strong>' }}ls{{ '</strong>' }}
job_id
process_id
process_start_time
$ {{ '<strong>' }}cat process_id{{ '</strong>' }}
2542
```

## Use a custom command-line tool in the component's shell {:#use-a-custom-command-line-tool-in-the-components-shell}

You can add custom command-line tools to a component's shell environment in the
following ways:

* [Add tools for a single session](#add-tools-for-current-session)
* [Make tools permanently available](#make-tools-permanently-available)

### Add tools for your current session {:#add-tools-for-current-session}

To temporarily add tools, use the `--tools` flag to pass their package URLs
to the `ffx component explore` command.

For example, this adds the `net-cli` toolset to the `/core/network` component's
shell for the current session:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /core/network --tools fuchsia-pkg://fuchsia.com/net-cli
Moniker: /core/network
$ {{ '<strong>' }}net help{{ '</strong>' }}
Usage: net <command> [<args>]
...
```

### Make tools permanently available {:#make-tools-permanently-available}

As a component owner, you can make specific tools always available to users. To
do this, add the tool package URLs to the `fuchsia.dash.launcher-tool-urls`
facet in your component manifest:

```json
<rest of component manifest above>
facets: {
    "fuchsia.dash.launcher-tool-urls": [ "fuchsia-pkg://fuchsia.com/magma-debug-utils" ],
},
```

When a user runs `ffx component explore` on this component, the command
automatically resolves and loads the tools from the manifest. The user must have
a package server running that can provide the specified tools, otherwise the
user will see a warning.

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore <moniker>
Moniker: <moniker>
Using tool URLs from component manifest: ["fuchsia-pkg://fuchsia.com/magma-debug-utils"]
$
```

## Run a command on the component's shell non-interactively {:#run-a-command-on-the-components-shell-non-interactively}

To run a command in the component's on-device shell non-interactively and receive
`stdout`, `stderr`, and exit code, use the `-c` (or `--command`) flag, for example:

```none {:.devsite-disable-click-to-copy}
[host]$ ffx component explore /bootstrap/archivist -c "cat /runtime/elf/process_id"
Moniker: /bootstrap/archivist
2542
```

## Appendices

### Why can't I see child components from the parent? {:#why-cant-i-see-child-components-from-the-parent}

Fuchsia does not allow accessing child components directly from the parent.
Previously, using knowledge of the component topology to access a child component's
capabilities made tools brittle and dependent on hard-coded paths that encoded
knowledge about the system topology.

Instead, the following alternatives are recommended:

-  Route capabilities explicitly from the child to the parent component.
-  Explore the child component itself.

### How is this different from ffx component run? {:#how-is-this-different-from-ffx-component-run}

The [`ffx component run`][ffx-component-run] command creates and starts a component
in a specified collection within the component topology. However, `ffx component run`
offers no interactive capabilities. On the other hand, `ffx component explore` allows
exploring any existing component in the topology interactively. In summary, you can use
`ffx component explore` to learn about a component you just created using
`ffx component run`.

### What is the namespace root in ffx component explore? {:#what-is-the-namespace-root-in-ffx-component-explore}

By default, the `ffx component explore` command creates a virtual file system at the
namespace root (`/`) that contains the following directories:

| Directory  | Description                                                   |
| ---------- | ------------------------------------------------------------- |
| `/.dash`   | Contains binaries needed by Dash.                           |
| `/exposed` | Contains all exposed capabilities.                            |
| `/ns`      | Contains the component's namespace, exactly as your component |
:            : would see it.                                                 :
| `/svc`     | Contains capabilities needed by Dash.                       |

If the target component is running on the device, the following directories are also
present:

Directory  | Description
---------- | ------------------------------------------------------------------
`/out`     | Contains all capabilities currently being served by the component.
`/runtime` | Contains debug information served by the component’s runner.

If the `--layout namespace` flag is set in `ffx component explore`, the shell's
namespace will match the component's namespace.

### Can I access Zircon handles or make FIDL calls using the Dash shell? {:#can-i-access-zircon-handles-or-make-fidl-calls-using-the-dash-shell}

This is not supported directly from the command interpreter.

### How do I file a feature request for ffx component explore? {:#how-do-i-file-a-feature-request-for-ffx-component-explore}

File all feature requests under the
[`ComponentFramework > Tools`][cf-tools-buganizer]{:.external} Issue Tracker
component.

<!-- Reference links -->

[dash]: https://manpages.debian.org/testing/dash/dash.1.html
[fx-shell]: https://fuchsia.dev/reference/tools/fx/cmd/shell
[component-moniker]: /docs/concepts/components/v2/identifiers.md#monikers
[component-url]: /docs/concepts/components/v2/identifiers.md#component-urls
[instance-id]: /docs/development/components/component_id_index.md
[ffx-component-run]: /docs/development/tools/ffx/workflows/start-a-component-during-development.md
[cf-tools-buganizer]: https://issues.fuchsia.dev/issues/new?component=1404287&template=0
