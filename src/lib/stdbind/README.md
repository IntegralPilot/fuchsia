# stdbind

`stdbind` is a pair of C++ and Rust libraries intended to make `bindgen` usage
around the C++ standard library more ergonomic: on the C++ side, there are basic
'adapter' types of known ABI for things like `std::optional`; and on the Rust
side, there are ABI-compatible Rust equivalents adapters for the corresponding
standard Rust type (e.g., for `Option` in the case of `std::optional`). The idea
is that one would use the adapters on the C++ side and then constrain `bindgen`
to use the Rust equivalents.

## Example usage

```gn
import("//build/rust/rustc_bindgen.gni)

rustc_bindgen_crate("my-bindings") {
    # ...

    # Uses <lib/stdbind/optional.h>
    headers = ["my-header.h"]

    type_blocklist = [ "stdbind::optional" ]
    raw_lines = [ "use stdbind::Optional as stdbind_optional;" ]
    deps = [ "//src/lib/stdbind:stdbind-rs" ]
}
```

## ABI Specification

### `optional`

The fixed-ABI adapters in C++ and Rust are `stdbind::optional<T>` and
`stdbind::Optional<T>`, respectively.

- **Discriminant Tag**: An 8-byte `uint64_t` / `u64` located at offset 0.
  - `0`: indicates absence
  - `1`: indicates presence
- **Padding**: Sufficient padding bytes after the tag to satisfy `alignof(T)`
  when `alignof(T) > 8`.
- **Payload**: The value of type `T` placed immediately following the tag (at
  offset 8 for types with alignment <= 8).
