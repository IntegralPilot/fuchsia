# `resetctl`
A utility to manipulate devices that publish the `fuchsia.hardware.reset.Reset`
protocol.

See //sdk/fidl/fuchsia.hardware.reset for further details about the FIDL
definition.

## Usage
The utility can be invoked via `ffx component explore`

```bash
ffx component explore <query> --tools fuchsia-pkg://fuchsia.com/resetctl
```
Where query is the component URL, moniker, or instance ID of the component that
either provides or consumes the `fuchsia.hardware.reset` protocol. For example:

```bash
ffx component explore aml-reset --tools fuchsia-pkg://fuchsia.com/resetctl
```

Alternatively, you can add the following like to your component's CML to avoid
the need for the `--tools` flag from above:
```
facets: {
        "fuchsia.dash.launcher-tool-urls": [ "fuchsia-pkg://fuchsia.com/resetctl" ],
    },
```

Once component explore has been invoked, the utility will search both the
incoming (`/svc`) and outgoing (`/out/svc`) directories for the service
instances. See `resetctl --help` for more information.

## Including the package in your build graph
First make sure that the package is included in your build graph:

Option 1: via `fx set`
```bash
fx set <your-board-and-arch> --with //src/devices/reset/bin/resetctl:resetctl-pkg
```

Option 2: via `fx add-test`
```bash
fx add-test //src/devices/reset/bin/resetctl:resetctl-pkg
```