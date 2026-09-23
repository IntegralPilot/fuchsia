# kprint

`kprint` is a zero-overhead, stack-efficient logging and formatting crate
designed for systems and embedded Rust code in the Zircon kernel environment.

It allows developers to write standard, ergonomic Rust format strings (`{}`)
while compiling invocations directly down to C `printf` / `snprintf` calling
conventions.

## Overview & Architecture

Standard Rust formatting (`core::fmt::write!`, `format_args!`, and `println!`)
relies heavily on `core::fmt::Formatter`, trait objects, dynamic dispatch
tables, and intermediate stack formatting buffers. In resource-constrained
environments or bare-metal kernels, this introduces unwanted binary bloat and
stack space overhead.

`kprint` solves this by performing compile-time format string translation,
single-evaluation argument binding, and direct FFI argument marshaling:
1. **Compile-Time AST Translation**: A procedural macro parses familiar Rust
   format strings (e.g., `"{:04x}"` or `"{:<10s}"`) and converts them into
   static, null-terminated C format string literals (`b"%04llx\0"` or
   `b"%-10.*s\0"`).
2. **Trait-Based Argument Marshaling**: Formatting arguments are bound once to
   local variables and converted via zero-cost backend traits (`AsKPrintStr`,
   `AsKPrintCString`, `AsKPrintSignedInt`, `AsKPrintUnsignedInt`,
   `AsKPrintPointer`).
3. **Zero Stack & Heap Allocation**: When formatting slices (`&str`, `&[u8]`,
   `&CStr`), `kprint` passes `(slice.len(), slice.as_ptr())` to C `%.*s`
   specifiers, avoiding copying or temporary null-terminator allocations.
4. **Single Evaluation Semantics**: Every format expression is evaluated
   strictly once, guaranteeing safe usage with side-effecting expressions.
5. **Direct C Backend**: Invocations compile directly into `printf` calls (or
   `snprintf` for `kformat!`), producing minimal instruction size and zero
   formatting runtime overhead.

## Usage

### Basic Printing

```rust
use kprint::{kprint, kprintln};

fn example() {
    let score = 42;
    let user = "alice";

    // Standard output without newline using captured variables
    kprint!("Connecting user {user:s} with ID {score:#06x}...");

    // Standard output with newline
    kprintln!("Connection established.");
}
```

### In-Memory Formatting (`kformat!`)

When you need to format into a stack buffer without allocating:

```rust
use kprint::kformat;

fn format_example() {
    let mut buffer = [0u8; 128];
    let result: &[u8] = kformat!(&mut buffer, "Status: {:#08X} ({})", 0x1A, true);
    assert_eq!(result, b"Status: 0X00001A (true)");
}
```

### Advanced Formatting Features

```rust
// Compile-time string concatenation
kprintln!(concat!("prefix_", "status: {}"), 42);

// Named and positional arguments
kprintln!("{0} + {1} = {0}", "a", "b");
kprintln!("{greeting:s}, {name:s}!", greeting = "hello", name = "world");

// Null-terminated C string pointers (*const c_char)
let c_ptr = c"kernel".as_ptr();
kprintln!("booting {:cs}", c_ptr);
```

## Supported Format Variations

The procedural macro automatically infers C format families from literal types,
cast expressions, or format specifiers:

| Rust Specifier / Type | Generated C Specifier | C ABI Argument Cast / Trait | Example Output |
| :--- | :--- | :--- | :--- |
| `{}`, `{:d}`, `i32`, `isize` | `%lld` | `AsKPrintSignedInt` (`c_longlong`) | `-12345` |
| `{:u}`, `u32`, `usize` | `%llu` | `AsKPrintUnsignedInt` (`c_ulonglong`) | `12345` |
| `{:x}`, `{:X}` | `%llx`, `%llX` | `AsKPrintUnsignedInt` (`c_ulonglong`) | `abcd`, `ABCD` |
| `{:#x}`, `{:#X}` | `0x%llx`, `0X%llX` | `AsKPrintUnsignedInt` (`c_ulonglong`) | `0x0`, `0x2a` |
| `{:#08X}` | `0X%06llX` | `AsKPrintUnsignedInt` (`c_ulonglong`) | `0X00001A` |
| `{:o}` | `%llo` | `AsKPrintUnsignedInt` (`c_ulonglong`) | `100` |
| `{:s}`, `&str`, `&[u8]`, `&CStr` | `%.*s` | `AsKPrintStr` (`c_int`, `*const c_char`) | `fuchsia` |
| `{:cs}`, `{:z}`, `*const c_char` | `%s` | `AsKPrintCString` (`*const c_char`) | `null-terminated` |
| `{:c}`, `char`, `u8` | `%.*s` | `AsKPrintChar` (`c_int`, `*const c_char`) | `Z`, `A`, `🦀` |
| `{:b}`, `bool` | `%.*s` | length (`4` or `5`), string pointer | `true`, `false` |
| `{:p}`, `*const T`, `usize` | `%p` | `AsKPrintPointer` (`*const c_void`) | `0x7ffee123` |
| `{:.2f}`, `f64` | `%.2f` | `c_double` | `3.14` |
