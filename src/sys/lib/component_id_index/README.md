# `component_id_index` library

This library parses, merges and validates component ID index files.
It provides a way to use a custom encoding format (e.g, JSON), along with
conversion to/from the FIDL schema.

This library is used by:

* `//tools/component_id_index`, a build tool that validates and merges
  component ID index files written in JSON5 into a single index, which is
  written out to a FIDL-encoded index file which is used by `component_manager`.
* `component_manager` which consumes a single FIDL-encoded component ID index
  file.
* Static analysis tools, such as Scrutiny.

See `//sdk/fidl/fuchsia.component.internal/component_id_index.fidl` for the FIDL
schema.

In order to keep the size of `component_manager` small, this library does not
directly depend on `serde_json` and `serde_json5`. Instead, this library accepts
a decoder implementation which helps decode a JSON/JSON5 string into an `Index`
data structure.
