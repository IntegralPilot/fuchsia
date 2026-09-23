# iquery

`iquery` - the Fuchsia Inspect API Query Toolkit

## Synopsis

```
iquery [--format <format>] <command> [<args>]
```

## Description

`iquery` is a utility program for inspecting component nodes exposed over the
[Inspect API]. It accepts a set of options and a command with
its respective options.

To prevent hard to debug issues in selectors where your shell is escaping some
character or others, it's recommended to always wrap selectors in single or
double quotes so your shell passes them as raw strings to iquery.

## Options

### `--format`

The format in which the output will be displayed.

Accepted formats:

- `text`: default, good for human reading
- `json`: good for machine reading

### `--help`

Prints usage information.

## Commands

### `list`

Lists all components (relative to the scope where the archivist receives events
from) of components that expose inspect.

For v1: this is the realm path plus the realm name.

For v2: this is the moniker without the instances ids.

Example usage:

```
$ iquery list
bootstrap/device_manager
core/archivist
...
```

#### `--component`

A fuzzy-search query that can include URL fragments and moniker fragments. Accompanying positional-
argument selectors should omit the component selector, as it will be generated from the search
results.

#### `--with-url`

Prints both the moniker and the URL with which the component was launched.

#### `--help`

Prints usage information about `list`.

### `list-files [<monikers...>]`

Lists all files that contain inspect data under the given `paths`.

The files that this command looks for are:

- `fuchsia.inspect.Tree`: A service file. The standard way inspect libraries
  export inspect data.
- `*.inspect`: VMO files with inspect data. The standard way the Dart inspect
  library exports inspect data.
- `fuchsia.inspect.deprecated.Inspect`: A service file. The standard way the Go
  library exports inspect data.

Example usage:

```
$ iquery list-files bootstrap/archivist bootstrap/driver_manager
bootstrap/archivist
  fuchsia.inspect.Tree
bootstrap/driver_manager
  class/display-coordinator/000.inspect
  class/input-report/000.inspect
  class/input-report/001.inspect
  class/misc/000.inspect
  class/pci-root/000.inspect
  class/pci/000.inspect
  class/sysmem/481.inspect
  driver_manager/driver_host/10171/root.inspect
  ...
```

#### `--help`

Prints usage information about `list-files`.

### `selectors [<selectors...>]`

Lists all available full selectors (component selector + tree selector).

If a component selector is provided, it’ll only print selectors for that component.

If a full selector (component + tree) is provided, it lists all selectors under the given node.

Example usage:

```
$ iquery selectors 'core/archivist:root/fuchsia.inspect.Health' 'core/timekeeper'
core/archivist:root/fuchsia.inspect.Health:start_timestamp_nanos
core/archivist:root/fuchsia.inspect.Health:status
core/timekeeper:root/current:system_uptime_monotonic_nanos
core/timekeeper:root/current:utc_nanos
core/timekeeper:root:start_time_monotonic_nanos
```

#### `--data`

A repeatable argument specifying a tree selector. A single positional argument should be used with
this flag. The positional argument must be a fuzzy-search query, that will be converted to a
moniker, and spliced onto the tree selectors to form complete diagnostics selectors.

If this is specified, the output only contains monikers for components whose URL contains the
specified name.

#### `--help`

Prints usage information about `selectors`


### `show [<selectors...>]`

Prints the inspect hierarchies that match the given selectors.

Example usage:

```
$ iquery show 'archivist.cm:root/fuchsia.inspect.Health' 'core/timekeeper'
core/archivist:
  root:
    fuchsia.inspect.Health:
      start_timestamp_nanos = 30305104656
      status = OK
core/timekeeper:
  root:
    start_time_monotonic_nanos = 30347000053
    current:
      system_uptime_monotonic_nanos = 61617527688648
      utc_nanos = 1591119246552989779
```

#### `--data`

A repeatable argument specifying a tree selector. A single positional argument should be used
with this flag. The positional argument must be a fuzzy-search query, that will be converted to
a moniker, and spliced onto the tree selectors to form complete diagnostics selectors.


#### `--file`

The filename we are interested in. If this is provided, the output will only
contain data from components which expose Inspect under the given file under
their out/diagnostics directory.


#### `--help`

Prints usage information about `show`.


[Inspect API]: /docs/development/diagnostics/inspect/README.md
