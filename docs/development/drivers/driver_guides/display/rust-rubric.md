# Rubric for display drivers written in Rust

## Numeric types

**Guideline:** Follow [the Rust guidance][rust-book-integers] to default to
`i32` and use explicitly sized signed and unsigned integer types when
appropriate.

**Explanation:** Most driver writers come from a C / C++ background, where
unsized signed integers are often used for arithmetic.

**Guideline:** Use `std::num::NonZero<T>` types when the constraint holds.

**Explanation:** Ensures we handle zero as a special case. Enables
[Option representation optimizations][rust-option-representation].

**Guideline:** Use `Option<std::num::NonZero<T>>` types when zero represents a
special case.

**Explanation:** Ensures we handle zero as a special case. Nudges us to handle
the special case higher in the stack and pass a bare non-zero type to lower
levels.

**Guideline:** Use `std::num::NonZero<usize>` for logical memory addresses that
will be turned into non-nullable pointers, and
`Option<std::num::NonZero<usize>>` for nullable pointers.

**Explanation:** Straightforward conclusion from the guidelines above. The
`Option` type forces code that uses nullable pointers to be explicit about the
null case handling. Using the created pointers is unsafe Rust, and is
discouraged in a later section.

**Guideline:** Use `zx_sys::zx_paddr_t` for CPU physical memory addresses that
are passed to / obtained from Zircon.

**Explanation:** Clearly communicates intended usage, matches the type expected
by Zircon APIs.

**Guideline:** Use `u32` (or `std::num::NonZero<u32>` if a non-zero assumption
applies) / `u64` (or `std::num::NonZero<u64>`) for CPU or device physical memory
addresses that will be written to registers.

Examples:

```rust
use std::num::NonZero;
use zx_sys::zx_paddr_t;

// Useful displays have at least one pixel.
let display_width: NonZero<u16>;

// Our FIDL APIs use 0 as an invalid ID.
let imported_image_id: Option<NonZero<u64>>;

// To be obtained from a memory pinning API.
let mut image_physical_address: zx_paddr_t;

// Will be written into a register.
let image_physical_address: NonZero<u32>;

// [`None`] when the plane is disabled.
let image_physical_address_reg_value: Option<NonZero<u32>>;
```

## Instance creation

**Guideline:** Default to naming factory functions `new()`. This default applies
to fallible and async functions.

**Explanation:** Matches current prevailing Rust usage.

Examples:

```rust
use zx;

struct Data {}

impl Data {
    pub async fn new() -> Result<Data, zx::Status> {
        // ...
    }
}
```

**Guideline:** Use the `try_new()` name for a fallible factory function when a
type also exposes an infallible factory function.

**Explanation:** Matches current prevailing Rust usage. (This is a rare case,
though.)

**Guideline:** Implement `From<OtherType>` or `TryFrom<OtherType>` for
infallible / fallible conversion from other types. Do not provide constructors
that effectively do type conversions.

**Explanation:** Helps distinguish between conversion and more involved instance
creation.

## FIDL bindings

**Guideline:** Exclusively use the `fidl_next` bindings.

**Explanation:** `fidl_next` bindings are required for the driver transport.
Standardizing on them saves (human and AI) developers from context-switching
between two bindings.

## Unused code

**Guideline:** Use `#[expect(dead_code)]` with an explanatory comment. Do not
use `#[allow(dead_code)]`.

**Explanation:** The `#[expect]` version is enforced by the compiler, and
therefore protected from falling out of date.

**Guideline:** Do not use the underscore (`_`) variable name prefix where
`#[expect(dead_code)]` can be used instead.

**Explanation:** The underscore prefix is equivalent to `#[allow(dead_code)]`,
so the reasoning above applies.

**Guideline:** Use the underscore expression where it applies. Explain the
reasoning behind discarding values (like `Result`) when it's not immediately
obvious.

**Explanation:** The underscore expression is a different language construct
from the underscore prefix in variable names.

Examples:

```rust
use fdf_component::{Driver, Node};
use zx;

struct DisplayDriver {
    // We must keep the Node alive for the lifetime of the driver.
    #[expect(dead_code)]
    device_node: Node,
}

impl Driver for DisplayDriver {
    async fn stop(&self) {
        // Intentionally ignoring failure during device shutdown. There's
        // nothing we can do at this point.
        let _ = fallible_function_that_logs();
    }
}

fn fallible_function_that_logs() -> Result<(), zx::Status> { /* ... */ }
```

## Representation

**Guideline:** Place `#[repr(...)]` attributes above all other attributes.

**Explanation:** Having the representation defined early makes it more likely
that macros operating on the structure use the correct data layout.

**Guideline:** Stick to the default representation (`rust`) unless the type's
in-memory representation must be fixed. The in-memory representation must be
fixed if and only if values are directly loaded from or stored into memory
shared between the driver and a different piece of software or hardware. Follow
the guidelines below to choose a non-default representation.

**Explanation:** The default representation maximizes the compiler's
opportunities for optimization. We can use the `rust` representation in shared
memory, when we're guaranteed that the memory is used by multiple instances of
the same compiled binary. We have to use a fixed memory representation when
there are multiple pieces of software (different binaries) or hardware (devices)
using the in-memory values.

**Guideline:** Use `#[repr(transparent)]` for Rust "newtypes" that need a fixed
in-memory representation.

**Explanation:** `#[repr(transparent)]` encodes the "newtype" intent. The
compiler enforces that the struct wraps a single non-zero-sized type field.

**Guideline:** Use `#[repr(C)]` for multi-field composite types that need a
fixed in-memory representation.

**Explanation:** `#[repr(C)]` encodes the intent to produce a composite type
with deterministic field offsets and alignments.

**Guideline:** For every type that needs a fixed in-memory representation, have
unit tests checking each type's size and alignment, and each type member's
offset.

**Explanation:** Translating from vendor documentation to Rust is non-trivial,
and we use tests to reduce the risk of errors.

**Guideline:** When specifying a custom representation, `#[derive()]` the
following traits: `Copy`, `Clone`, `zerocopy::FromBytes`, `zerocopy::Immutable`,
`zerocopy::IntoBytes`, `zerocopy::KnownLayout`.

**Explanation:**

- `Copy` makes it easy to reason about pointer operations
- `Clone` is required by `Copy`
- `zerocopy::FromBytes` proves that the type can be used to read any bit pattern
- `zerocopy::FromZeros` is implied by `zerocopy::FromBytes`
- `zerocopy::Immutable` proves that the type does not use interior mutability
- `zerocopy::IntoBytes` proves that the type can be treated as a sequence of bytes
- `zerocopy::KnownLayout` is required by other `zerocopy` derived traits

Examples:

```rust
use bitfield::bitfield;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

bitfield! {
    #[repr(transparent)]
    #[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
    struct CommandFlags(u32) {}
}

#[repr(C)]
#[derive(Copy, Clone, FromBytes, Immutable, IntoBytes, KnownLayout)]
struct Command {
    pub flags: CommandFlags;
    pub id: u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{align_of, offset_of, size_of};

    #[fuchsia::test]
    fn test_command_abi() {
        assert_eq!(size_of::<Command>(), 8);
        assert_eq!(align_of::<Command>(), 4);
        assert_eq!(offset_of!(Command, flags), 0);
        assert_eq!(offset_of!(Command, id), 4);
    }
}
```

## `use` paths

**Guideline:** Follow the official defaults for idiomatic `use` paths:

- Bring into scope: types, derive and attribute-like macros
- Bring parent module into scope: functions, function-like macros

**Explanation:** Matches the established recommendation in
[The Rust Programming Book section on idiomatic use paths][rust-book-idiomatic-paths].

**Guideline:** Bring into scope the top-level module of the `zx` crate.

**Explanation:**
The `zx` crate exports generic type names such as `Channel` and `Event`, which
were intended to be read with a `zx::` prefix -- for example, `zx::Event`
reads as "Zircon event". This is an intentional deviation from
[the Rust Programming Book section on idiomatic use paths][rust-book-idiomatic-paths],
which recommends bringing into scope the parent modules for both types involved
in a name conflict.

**Guideline:** Bring into scope the parent module for register or ABI definition
types. When it makes sense, alias the modules as `abi` or `registers`.

**Explanation:** Similar reasoning to the rule above. We deviate from the Rust
convention because register and ABI type names are likely to overlap Rust driver
type names, as they cover the same domain. We reuse the practice in C++ drivers
that gave us a good tradeoff between clarity and conciseness.

**Guideline:** Alias each `fidl_next` binding module. Use the `fidl_` prefix for
all aliases. Use the alias to qualify access to both structs and functions.

**Explanation:** Same reasoning as above. FIDL and Rust driver type names are
likely to overlap.

Examples:

```rust
use fidl_next_fuchsia_sysmem2 as fidl_sysmem2;
use fidl_next;

pub async fn use_buffer_collection(
    sysmem_buffer_collection: &mut fidl_next::Client<fidl_sysmem2::BufferCollection>,
) {
  /* ... */
  log::warn!("Failed to get hardware pixel formats, falling back to safe set");
  /* ... */
}
```

## MMIO region management

**Guideline:** Use the `MmioRegion<VmoMemory, Arc<VmoMemory>>` type for all MMIO
*memory regions.

**Explanation:** `std::sync::Arc` meets the `MmioSplit` trait constraints,
allowing any module to further subdivide the MMIO region it receives. `Arc` is
compatible with any threading model (as opposed to `Rc`). The atomic overhead is
negligible, because the reference count is only changed during driver start and
stop.

**Guideline:** Use `split_off`. Do not use `try_split_off`.

**Explanation:** Hardware presents well-known static MMIO maps. The driver code
for building the maps should not need any conditional logic.

Examples:

```rust
use fidl_next_fuchsia_hardware_platform_device as fidl_platform_device;
use mmio::MmioSplit;
use mmio::region::MmioRegion;
use mmio::vmo::VmoMemory;
use std::sync::Arc;

/// Obtains an MMIO region from the Platform Device.
///
/// Logs on failure.
async fn map_mmio_range(
    platform_device: &fidl_next::Client<fidl_platform_device::Device>,
    range_name: &str,
) -> Result<MmioRegion<VmoMemory, Arc<VmoMemory>>, zx::Status> {
    let region = platform_device.map_mmio_by_name(range_name).await.map_err(|err| {
        log::error!("Failed to map MMIO range {range_name}: {err:?}");
        err.log_to_status()
    })?;
    Ok(region.into_split_send())
}

impl FunctionalUnit {
    pub fn new(mmio: MmioRegion<VmoMemory, Arc<VmoMemory>>) {
        // Subunit 1 manages the MMIO range 0x0000..0x1000.
        let subunit1_mmio = mmio.split_off(0x1000);
        let subunit1 = SubUnit1::new(subunit1_mmio);

        // Subunit 2 manages the MMIO range 0x1000..0x2000.
        let subunit2_mmio = mmio.split_off(0x1000);
        let subunit2 = SubUnit1::new(subunit1_mmio);

        // Subunit 3 manages the MMIO range from 0x2000 onwards.
        let subunit3 = SubUnit3::new(mmio);

        Self { subunit1, subunit2, subunit3 }
    }
}
```

## Pointers and shared memory

**Guideline:** Use pointers to access memory that is shared with hardware.

**Explanation:** The Rust memory model’s assumptions trigger on reference creation,
and do not require references to be accessed.

**Guideline:** Immediately convert a `usize` returned by a system call to a
`std::num::NonZero<usize>`.

**Explanation:** Forcing function to assert that the system call returns a
non-null pointer on success.

**Guideline:** When a pointer’s target can be expressed by a Rust type,
immediately convert `std::num::NonZero<usize>` into a `std::ptr::NonNull` to the
type.

**Explanation:** Minimize the potential for errors.

**Guideline:** When pointer arithmetic is necessary, immediately convert
`std::num::NonZero<usize>` into `std::ptr::NonNull`, then use methods like
`add()` / `byte_add()` and `cast()`.

**Explanation:** Reduced potential for errors. Pointer provenance is maintained.

Examples:

```rust
use std::num::NonZero;
use std::ptr::NonNull;
use zx;

#[repr(...)]
#[derive(...)]
struct Header { /* ... */ }

#[repr(...)]
#[derive(...)]
struct Trailer { /* ... */ }

struct SharedMemory {
    header_ptr: NonNull<Header>,
    trailer_ptr: NonNull<Trailer>,
}

impl SharedMemory {
    pub fn new() -> Result<Self, zx::Status> {
        let data_address = fuchsia_runtime::vmar_root_self().map(...)?;
        let data_address = NonZero::<usize>::new(data_address)
            .expect("zx::vmar::map() returned null address");

        // [`Option::unwrap()`] is guaranteed not to panic. The [`NonZero::new()`]
        // call above already checked that the pointer is non-null.
        let header_ptr = NonNull::new(
            std::ptr::with_exposed_provenance_mut(data_address.get())
        ).unwrap();

        // SAFETY: The memory allocation covers both [`Header`] and [`Trailer`].
        let trailer_ptr = unsafe { header_ptr.add(1) }.cast::<Trailer>();

        Ok(Self { header_ptr, trailer_ptr })
    }
}
```

## Logging

**Guideline:** Only use the ERROR level to report conditions caused by bugs in
Fuchsia. Do not assume that the ERROR level is always appropriate when returning
an error.

**Explanation:** Corrects a popular misunderstanding of [RFC-0003][logging-rfc].

**Guideline:** Use the WARNING level to report hardware failures that can occur
assuming correct driver operation.

**Explanation:** Follows from RFC-0003.

**Guideline:** Only use the INFO level to provide context for current
investigations. Precede the logging statement with a comment that includes a
link to the investigation's tracking issue.

**Explanation:** Driver logs at INFO or above are included in the kernel serial
log, which is used to assess overall device health.

**Guideline:** Do not log at INFO or above on high-frequency code paths.
Use [Inspect][inspect-readme] for data that changes often.

**Explanation:** Same reasoning as above.

Examples:

```rust
use zx;

/// Errors if the hardware returns an invalid version.
///
/// All error conditions are logged.
pub fn read_version() -> Result<u32, zx::Status> {
    debug!("read_version()");

    let version_value: u32 = read_from_register();

    // TODO(https://fxbug.dev/12345678): We suspect that the crashes are
    // correlated with specific hardware versions. Reduce the log level to DEBUG
    // after proving or disproving the hypothesis.
    info!("Component version: {}", version_value);

    if version_value == 0 {
        log::warn!("Invalid version, device may be off: {}", version_value);
        // ...
    }
    // ...
}
```

## Use the try operator

**Guideline:** Prefer the try operator (`?`) over all other error handling
alternatives. Prioritize your callers’ ability to use the try operator when
designing function interfaces.

**Explanation:** Concise error handling lets readers focus on the higher-level
picture. The try operator matches the error handling in C++ display drivers,
where `zx::result<>` errors are bubbled up the stack.

**Guideline:** Design the errors used in returned value types to facilitate the
use of `?`. In particular, prefer using `zx::Status` as the error type in a
`Result`.

**Explanation:** For now, follow the same error handling approach as the C++
display drivers.

**Guideline:** Clearly document when a function logs a condition that produces
an error result.

**Explanation:** Developers who are assured that the condition is logged can use
the try operator. This produces more concise code, and reduces redundant
logging.

Examples:

```rust
use zx;

/// Errors if the hardware returns an invalid version.
///
/// All error conditions are logged.
pub fn read_version() -> Result<u32, zx::Status> { /* ... */ }

/// Initializes the hardware so it can receive commands.
///
/// All error conditions are logged.
pub fn initialize_hardware() -> Result<(), zx::Status> {
    let version_value = read_version()?;

    /* ... */
}
```

## Naming: length vs size vs capacity

**Guideline:** Use `length` to name variables that count the number of elements
in a collection. Use `len()` for functions.

**Explanation:** Matches Rust common practice, such as `Vec::len()`.

**Guideline:** Use `size_bytes` to name variables and functions that count the
number of bytes used to store or transmit something. Do not create functions
that would be redundant with invocations of `core::mem::size_of`.

**Explanation:** "Size" is idiomatically used in Rust to name this concept.
However, the "size" term shows up a lot in hardware-related documents. The
"\_bytes" suffix helps disambiguate.

**Guideline:** Use `capacity` to name variables and functions that report the
maximum number of elements supported by the memory backing a collection. In
particular, a collection whose backing storage never changes has fixed capacity,
and its length changes as elements are inserted and removed.

**Explanation:** Matches Rust common practice, such as `Vec::capacity`.

## Links in documentation

**Guideline:** Link all identifiers supported by
[Rustdoc’s link-by-name feature][rustdoc-link-by-name].

**Explanation:** Recommended by
[Rust API guidelines on documentation][rust-api-guidelines-documentation-links].
[rust-analyzer][rust-analyzer] can navigate the links.

Curated list of disambiguator prefixes that suggest what Rustdoc can link to:
value, constant, primitive, module, function, type, typealias, struct, field,
method, trait, enum, variant, union, macro, derive.

Example:

```rust
/// See [`DeviceBuilder`] for obtaining instances.
pub struct Device {}
```

## Minimize nesting level

**Guideline:** After calling a fallible function, immediately check for errors,
optionally log the error, and return.

**Explanation:** Human reviewers prefer reading a process laid out as a sequence
of steps, rather than nested conditional blocks.

**Guideline:** Extract non-trivial error handling to a dedicated function.
Trivial error handling is logging and returning.

**Explanation:** Human reviewers have an easier time analyzing functions that
implement a single process.

## Field visibility

**Guideline:** Fields on composite types must be all private, or all public.
Public means `pub` or `pub(crate)`.

**Explanation:** Public fields are not amenable to invariants.

## Synchronization

**Guideline:** Use the `fuchsia_sync` implementations of the following types:
`Condvar`, `Mutex`, `RwLock`. Do not use the variants in `std::sync` or
`parking_lot`.

**Explanation:** The `fuchsia_sync` implementations perform deadlock detection
in `debug` builds.

## Prose writing style

**Guideline:** One idea per sentence.

**Explanation:** AI agents aim to make minimal local edits, which build up to
run-on sentences.

**Guideline:** Three or more parallel items go in a list or a table. Only use a
table when the items have attributes worth aligning.

**Explanation:** Concrete fix for major cause of run-on sentences.

**Guideline:** Rationale gets its own sentence or paragraph. Follow progressive
disclosure: rule first, followed by rationale, followed by evidence.

**Explanation:** Concrete fix for major cause of run-on sentences.

**Guideline:** Limit historical information to evidence backing current
rationale explanations. Avoid narrative style elements: unnecessary incident
details, dates, plan iteration IDs. Delete superseded decisions and facts that
are no longer relevant.

**Explanation:** AI agents tend to build mini-archives of historical information
in comments and documents, instead of relying on version history for archival.

**Guideline:** State each fact once, in the most relevant place. Use references
instead of duplicating.

**Explanation:** Same as above.

**Guideline:** Aim for terseness, within the bounds of remaining clear. Use
present tense and precise technical language. Avoid subjective details -- most
adjectives are subjective.

**Explanation:** Tunes the AI agent tone to balance token effectiveness and
helpfulness.

## Additional guides

This document focuses on issues commonly encountered while reviewing code
produced by AI agents.

Display drivers also follow the best practices below.

- [The Rust API Guidelines][rust-api-guidelines]{.external}
- [The Rust Style Guide][rust-style-guide]{.external} as implemented by `fx format-code`
- [The rustdoc book][rustdoc-howto]{.external}
- [The Rustonomicon][rustonomicon]{.external} (mostly informative, covers `unsafe` code)

[google-cpp-style-integers]: https://google.github.io/styleguide/cppguide.html#Integer_Types
[inspect-readme]: /docs/development/diagnostics/inspect/README.md
[logging-rfc]: /docs/contribute/governance/rfcs/0003_logging.md
[rust-analyzer]: https://rust-analyzer.github.io/
[rust-api-guidelines]: https://rust-lang.github.io/api-guidelines/
[rust-api-guidelines-documentation-links]: https://rust-lang.github.io/api-guidelines/documentation.html#prose-contains-hyperlinks-to-relevant-things-c-link
[rust-book-idiomatic-paths]: https://doc.rust-lang.org/book/ch07-04-bringing-paths-into-scope-with-the-use-keyword.html#creating-idiomatic-use-paths
[rust-book-integers]: https://doc.rust-lang.org/book/ch03-02-data-types.html#integer-types
[rust-option-representation]: https://doc.rust-lang.org/std/option/#representation
[rust-style-guide]: https://doc.rust-lang.org/style-guide/
[rustdoc-howto]: https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html
[rustdoc-link-by-name]: https://doc.rust-lang.org/rustdoc/write-documentation/linking-to-items-by-name.html
[rustonomicon]: https://doc.rust-lang.org/nomicon/
