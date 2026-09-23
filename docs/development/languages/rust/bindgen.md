# Integrating C/C++ using `bindgen`

If you need to call some C or C++ APIs from Rust, you can use [`bindgen`] which generates Rust code
from C & C++ headers. For more documentation, see [the `bindgen` User
Guide](https://rust-lang.github.io/rust-bindgen/).

## Generating Rust bindings

Fuchsia provides the `rustc_bindgen_golden` GN template to generate Rust bindings from C/C++ headers
and ensure they remain up-to-date. This template runs `bindgen` during the build and compares the
output against a checked-in "golden" file.

### 1. Define the GN target

Import the template and define a `rustc_bindgen_golden` target in your `BUILD.gn`.

For example, see [`//src/lib/usb_rs/BUILD.gn`](/src/lib/usb_rs/BUILD.gn):

```gn
import("//build/rust/rustc_bindgen.gni")

rustc_bindgen_golden("my_bindings_golden") {
  header = "my_header.h"
  checked_in_source = "src/bindings.rs"

  # Optional: configure bindgen behavior
  # e.g., allowlist, denylist, raw_lines, etc.
  # see //build/rust/rustc_bindgen.gni for all options
}
```

### 2. Configure the Rust target

You must add the golden target to the `validations` parameter of the Rust target that uses the
bindings to ensure the check runs as part of the build:

```gn
rustc_library("my_library") {
  edition = "2024"
  sources = [
    "src/lib.rs",
    "src/bindings.rs",
  ]
  # ...

  validations = [ ":my_bindings_golden" ]
}
```

In your Rust code, you can then use the generated bindings (e.g., `mod bindings;`).

### 3. Updating the checked-in bindings

If the C header changes, the build will fail because the generated bindings will no longer match the
checked-in file. The build output will display a diff and instructions on how to update the file.

To update the checked-in file, you can either:

* Copy the generated file over the checked-in file using the `cp` command provided in the build
  failure message.
* Rebuild with the `update_goldens` GN argument set to `true`:

  ```bash
  fx set ... --args=update_goldens=true
  fx build
  ```

  After the build completes and updates the files, revert the `update_goldens` argument to `false`
  (or remove it) to ensure future mismatches are caught.

[`bindgen`]: https://github.com/rust-lang/rust-bindgen
