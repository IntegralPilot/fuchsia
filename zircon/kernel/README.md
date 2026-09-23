# The Zircon build

## Kernel switchsets, or how the kernel is compiled and linked

- **What**: A suite of kernel-specific GN toolchains parameterized by CPU
  architecture, representing compiler and linker flags (e.g., optimization
  levels, debug levels, asserts, defines).

- **Where in source**: Switchsets are defined at
  `//zircon/kernel/switch/set/$name:kernel_$cpu.$name` using the
  [kernel_switchset()](/zircon/kernel/kernel_switchset.gni) template.

- **Where in the build directory**: The compilation artifacts for a given
  switchset will be placed under `<build_dir>/kernel_$cpu.$name/`

- **Constraint**: A switchset must be _defined_ within the default toolchain.

**Current switchsets**:

- [lk_debug_level_0](/zircon/kernel/switch/set/lk_debug_level_0/BUILD.gn):
  Production-like configuration with minimal debug-like features, setting
  `LK_DEBUGLEVEL=0`;
- [lk_debug_level_2](/zircon/kernel/switch/set/lk_debug_level_2/BUILD.gn):
  Development-like configuration with full debug-like features, setting
  `LK_DEBUGLEVEL=2`.

## Kernel executables

- **What**: ELF-producing targets representing the kernel binary itself. Defines
  what is included in the kernel link (e.g., tests and debug commands).

- **Where in source**: Defined in `//zircon/kernel/bin/BUILD.gn` using the
  [kernel_executable()](/zircon/kernel/bin/BUILD.gn) template.

- **Where in the build directory**: The unstripped ELF binary compiled under a
  given switchset is placed at `<build_dir>/kernel_$cpu.$switchset/$executable`.

- **Constraint**: Must only be evaluated within a kernel switchset toolchain.

**Current kernel executables**:

- `vmzircon`: Standard kernel intended for production (no tests or extra debug
  commands). This is what is packaged in user-like products.
- `vmzircon.with-tests`: Standard kernel intended for development (compiled with
  unit tests and additional debug commands). This is what is packaged in eng
  products.

## Kernel Images (A.K.A. kernel ZBIs)

- **What**: Minimal bootable ZBIs containing a kernel (by definition) and
  everything needing to boot it. They package a kernel executable (evaluated
  under a specific switchset), along with things like boot options, code
  patching inputs, userboot, and a vDSO.

- **Where in source**: Defined at `//zircon/kernel/image/$name:$name` using the
  [kernel_image()](/zircon/kernel/kernel_image.gni) template

- **Where in the build directory**: The ZBI is placed at
  `<build_dir>/kernel.$name.zbi`.

**Current kernel images**:

- [eng](/zircon/kernel/image/eng/BUILD.gn): Assembled into eng products. Uses
  `vmzircon.with-tests` under the `lk_debug_level_2` switchset, with
  debug/serial syscalls enabled.

- [user](/zircon/kernel/image/user/BUILD.gn): Assembled into user products. Uses
  `vmzircon` under the `lk_debug_level_0` switchset, with serial/debug syscalls
  disabled.

- [userdebug](/zircon/kernel/image/userdebug/BUILD.gn): Assembled into userdebug
  products. Uses `vmzircon` under the `lk_debug_level_0` switchset, but with
  serial output enabled.

- [eng.lk_debug_level_0](/zircon/kernel/image/eng.lk_debug_level_0/BUILD.gn):
  Eng-like test image to verify that debug assertions are not load-bearing. Uses
  `vmzircon` under `lk_debug_level_0`.

## Source organization

While it might well be more convenient to have consolidated more of these
definitions in fewer build files, the current organization is intended to
minimize the amount of unnecessary GN evaluation that happens at `gen` time.
(Recall that if GN sees a redirect to another toolchain in a build file, it will
follow it, and so on.)

- **Switchsets** are isolated in subdirectories so they are only evaluated when
  explicitly referenced.

- **Executables** are centralized in `//zircon/kernel/bin/BUILD.gn` for
  readability and to ensure they are only evaluated under kernel switchsets.

- **Images** are separated to allow different images to depend on different
  switchsets without accidentally referencing unused switchsets.

## FAQ

### Which kernel does Product Assembly end up using?

Product Assembly knows about three particular kernel images: `eng`, `user`, and
`userdebug`, and only by label. Its contract with the kernel build is simply
that it will pick out the appropriate kernel image based based on a product's
build type.

### Which kernel do core tests end up running against?

By default, 'kernel ZBI tests' (e.g., core tests) run against the `eng` kernel
image, which in turn packages the `vmzircon.with-tests` executable under the
`lk_debug_level_2` switchset. To run these tests against additional kernel
images (e.g., `eng.lk_debug_level_0`), set the `extra_kernel_test_images` GN arg
in your `args.gn`:

```gn
extra_kernel_test_images = [ "eng.lk_debug_level_0" ]
```

Generally, this will cause
[kernel_zbi_test()](/build/testing/boot_tests/kernel_zbi_test.gni) targets to
also generate test variants for the specified images (e.g.,
`path/to/test:test.user` alongside `path/to/test:test.eng`), which can then be
run via `fx run-boot-test`. The above example is what all bringup builders set.

For core tests in particular, one may run
`fx core-tests --kernel=$kernel_image_name` to locally run core tests against a
given kernel image under QEMU. Here `$kernel_image_name` must be one of `eng`
(the default) or an entry in your local `extra_kernel_test_images`.

### How to enable boot options in core tests only?

`//zircon/kernel/image/$name:$name.test-data-deps` are data dependencies that
every instance of
[kernel_zbi_test()](/build/testing/boot_tests/kernel_zbi_test.gni) (e.g., core
tests) includes. This target may be updated to depend on desired
[`kernel_cmdline()`](/build/zbi/kernel_cmdline.gni) instances to enable
test-only boot options.

### How to disassemble the kernel?

It is most practical to run `fx dis vmzircon` or `fx dis vmzircon.with-tests` to
disassemble `vmzircon` or `vmzircon.with-tests` across all switchsets in your
build. One can also run `fx dis 'vmzircon*'` include both executables.

### I just want to build the kernel

There is no longer a singular 'kernel' to build. For a more minimal build while
iterating on kernel build, it is recommended that one build kernel images
directly (e.g., `fx build '//zircon/kernel/image/eng'` or
`fx build -- kernel.eng.zbi`).

# Rust Code Organization in the Zircon Kernel

## Single Crate Architecture

The Zircon kernel is compiled as a single, monolithic Rust executable crate
(`--crate-name=kernel`), rooted at [`zircon/kernel/main.rs`](/zircon/kernel/main.rs).

Key properties:
- **`#![no_std]` and `#![no_main]`**: The kernel operates in a bare-metal
  environment without standard library runtime support or default entry points.
- **Checked-in Root**: `main.rs` explicitly declares the kernel's top-level submodules using
  `#[path = "..."]` attributes (for example, `#[path = "kernel/mod.rs"] pub mod kernel;`).
- **Conditional Compilation**: Target-specific modules (e.g. `platform_pc` on
  `x86_64`) and test modules (under `#[cfg(ktest)]`) are conditionally declared
  in `main.rs`.

## Module Organization & Hierarchy

Rust source code within the kernel follows a hierarchical module structure:

```
zircon/kernel/
├── main.rs                 # Root of the kernel crate
├── kernel/                 # Core kernel primitives (threads, sync, scheduler)
│   ├── mod.rs              # pub mod thread; pub mod mp; ...
│   ├── thread.rs
│   └── ...
├── vm/                     # Virtual memory subsystem
│   ├── mod.rs              # Declares VM submodules & external module paths
│   ├── page_slab_allocator.rs
│   └── ...
├── lib/
│   ├── heap/               # Kernel heap library (e.g., heap.rs)
│   ├── user_copy/          # User-memory copying primitives
│   └── ...
└── platform/               # Architecture- and board-specific code
```

The kernel is a mix of C++ and Rust code. The directory structure follows the structure of the C++
kernel. When the conversion to Rust is complete, we will restructure the code to more closely match
Rust conventions.

### Subsystem Modules (`mod.rs`)

Each major kernel subsystem typically has a `mod.rs` file that defines its
public API and internal modules:

- Standard intra-directory modules are declared normally (e.g., `pub mod thread;`
  in `zircon/kernel/kernel/mod.rs`).
- Modules defined in other directories (such as under `zircon/kernel/lib/`) that
  are consumed as submodules of a subsystem are declared using relative `#[path]`
  attributes. For example, `zircon/kernel/vm/mod.rs` declares the `heap` module:

  ```rust
  #[path = "../lib/heap/heap.rs"]
  pub mod heap;
  ```

  This makes the module available as `crate::vm::heap`.

## GN Build Templates & Infrastructure

Because all kernel Rust code is compiled into a single crate, GN targets do not
compile separate `.rlib` archives for each kernel submodule. Instead, GN
templates coordinate source tracking and dependency propagation.

### `kernel_rust_mod`

Defined in [`//zircon/kernel/kernel_rust_mod.gni`](/zircon/kernel/kernel_rust_mod.gni).

Represents a logical Rust module within the kernel crate. Example:
```gn
kernel_rust_mod("heap-rs") {
  sources = [ "heap.rs" ]
  deps = [ ":heap-bindings" ]
}
```

Note: **Always list all `.rs` files in `sources`**. Omitting a file from
  `sources` will cause depfile verification to fail during the build.

### `kernel_rust_crate`

Defined in [`//zircon/kernel/kernel_rust_crate.gni`](/zircon/kernel/kernel_rust_crate.gni).

Defines an external Rust library crate (`rustc_library`) built specifically for the kernel environment.

### `kernel_bindgen_crate`

Defined in [`//zircon/kernel/kernel_bindgen_crate.gni`](/zircon/kernel/kernel_bindgen_crate.gni).

Generates Rust FFI bindings from C/C++ kernel headers using `rustc_bindgen` and exposes them as an
external crate. Example:
```gn
kernel_bindgen_crate("heap-bindings") {
  headers = [ "include/lib/heap_ffi.h" ]
  non_rust_deps = [ ":heap" ]
  output_name = "heap_bindings.rs"
  cpp = true
}
```
In Rust code:
```rust
use heap_bindings as bindings;
```

## How to Manage Libraries and Dependencies

### Adding a New Source File to an Existing Module

1. Add the `.rs` file to `sources` in the corresponding `kernel_rust_mod` target
   in `BUILD.gn`:
   ```gn
   kernel_rust_mod("kernel-rs") {
     sources = [
       ...
       "new_feature.rs",
     ]
   }
   ```
2. Declare the submodule in `mod.rs`:
   ```rust
   pub mod new_feature;
   ```

### Wiring Up a Library Module (`kernel_rust_mod`)

When a subsystem needs to use a Rust module located in another directory (for
instance, a library in `//zircon/kernel/lib/<libname>`):

1. **Define the library module**: Ensure `//zircon/kernel/lib/<libname>/BUILD.gn`
   defines a `kernel_rust_mod` listing its source files:
   ```gn
   kernel_rust_mod("<libname>-rs") {
     sources = [ "<libname>.rs" ]
     deps = [ ... ]
   }
   ```
2. **Add GN dependency**: In the consumer's `BUILD.gn`, add the library target to
   the consumer's `kernel_rust_mod` `deps`:
   ```gn
   kernel_rust_mod("vm-rs") {
     ...
     deps = [
       ...
       "//zircon/kernel/lib/<libname>:<libname>-rs",
     ]
   }
   ```
3. **Declare the module in Rust**:
   - If the library belongs under a specific subsystem (e.g., `crate::vm::<libname>`),
     declare it in that subsystem's `mod.rs`:
     ```rust
     #[path = "../lib/<libname>/<libname>.rs"]
     pub mod <libname>;
     ```
   - If the library is a top-level kernel module (e.g., `crate::<libname>`),
     declare it in `zircon/kernel/main.rs`:
     ```rust
     #[path = "lib/<libname>/<libname>.rs"]
     pub mod <libname>;
     ```

### Force-Linking C Entry Points

If an external crate provides `extern "C"` functions that are called by C++
code but not referenced directly by Rust code, Rust's dead-code elimination may
strip the crate. To prevent this, add an explicit reference in
`zircon/kernel/main.rs`:

```rust
use <crate_name> as _;
```

## Testing

- **In-Kernel Unit Tests**: Kernel unit tests use `#[cfg(ktest)]` and the
  `#[unittest::suite]` macro from the kernel `unittest` framework.
- **Test Dependencies**: Add test-only dependencies to `test_deps` in
  `kernel_rust_mod`.
- **Running Tests**:
  - Run with `fx core-tests --kernel-unittest <suite_name>` or
    `fx run-boot-test core-tests.eng`.
