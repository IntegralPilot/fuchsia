# serve_processargs

`serve_processargs` is a Rust library for serving a processargs representation
of a bedrock dictionary. Examples:

- When a `Receiver` capability in the dictionary has values, it will send
  requests to the `fuchsia.io/Directory` server endpoint representing the
  outgoing directory (`PA_DIRECTORY_REQUEST`).
- It will monitor requests on `fuchsia.io/Directory` client endpoints
  representing namespace entries (`PA_NS_DIR`), and translate them to values in
  the corresponding `Sender` capabilities.
- Other handles in `processargs` which the Elf runner doesn't use as an
  implementation detail.

## Building

To add this component to your build, append
`--with src/sys/bedrock/serve_processargs:tests`
to the `fx set` invocation.

## Testing

Tests for `serve_processargs` are available in the `serve_processargs_unittests`
package.

```sh
fx test serve_processargs_unittests
```
