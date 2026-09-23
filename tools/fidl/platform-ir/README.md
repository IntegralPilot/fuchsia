# Platform IR

This is a tool to combine the FIDL JSON IR from multiple different library IRs
into a single one. This is most useful for capturing a whole Fuchsia platform's
IR in a single file.

The JSON that this tool produces differs from the IR produced by fidlc, consumed
by fidlgens and defined in `tools/fidlc/schema.json`. It omits the `name` and
`maybe_attributes` at the top-level since these are per-library. It omits
`library_dependencies` since this IR must not have any external dependencies and
`declaration_order` because it's more trouble than it's worth.

It's an error for the tool to receive more than one input with the same library
defined (ie: duplicate top-level `name`). It's an error for inputs to have
different `experiments` enabled. All referenced declarations must be provided.