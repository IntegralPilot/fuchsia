# Porting process

## Acceptance criteria

- Previously used `//src/lib/fidl/rust`, now uses `//src/lib/fidl/rust_next`
- Previously used `fidl_fuchsia_<FIDL name>` crates, now uses `fidl_next_fuchsia_<FIDL name>`

## Necessary changes

- Add `//src/lib/fidl/rust_next/fidl_next` as a dependency
- Add your porting target to the allowlist in `//src/lib/fidl/rust_next/fidl_next/BUILD.gn`
    - Look for `# NOTE: these bindings are currently in a phased rollout` visibility list
- Add `enable_rust_next` to the FIDL targets you're using

## Examples

Compare:
- Current bindings: `//examples/fidl/calculator/rust`
- New bindings: `//examples/fidl/calculator/rust_next`
- FIDL file: `//examples/fidl/calculator/fidl/calculator.test.fidl`
    - Build file: `//examples/fidl/calculator/fidl/BUILD.gn`

## New bindings documentation

- Overall crate: `https://fuchsia-docs.firebaseapp.com/rust/fidl_next/index.html`
    - This is what you want to depend on, it re-exports the separate pieces of the FIDL bindings (codec, protocol, type-safe bindings helpers)
- Codec docs: `https://fuchsia-docs.firebaseapp.com/rust/fidl_next_codec/index.html`
    - Has an explanation of how the codec layer is structured
- Protocol docs: `https://fuchsia-docs.firebaseapp.com/rust/fidl_next_protocol/index.html`
    - Has an explanation of how the protocol layer is structured
