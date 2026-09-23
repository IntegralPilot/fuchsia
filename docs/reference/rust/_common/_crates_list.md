## General

* [fuchsia](https://fuchsia-docs.firebaseapp.com/rust/fuchsia/index.html)

    Macros for creating Fuchsia components and tests. These macros work on
    Fuchsia, and also on host with some limitations (that are called out where
    they exist).

* [fuchsia_component](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_component/index.html)

    Support library for implementing Fuchsia components.

* [fuchsia_component_test](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_component_test/index.html)

    The Realm Builder library exists to facilitate integration testing of
    components by allowing for the run-time construction of realms and mocked
    components specific to individual test cases. For more information on how
    to use this library, see
    [Realm Builder](/docs/development/testing/components/realm_builder.md#rust)

* [fuchsia_async/](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/index.html)

    Futures executor and asynchronous primitives (Channel, Socket, Fifo, etc.).
    This crate works on Fuchsia and host Operating Systems and allows you
    to use Overnet for RCS and ffx.

* [async_utils](https://fuchsia-docs.firebaseapp.com/rust/async_utils/index.html)

    Provides utilities for working with asynchronous code.

* [async_helpers](https://fuchsia-docs.firebaseapp.com/rust/async_helpers/index.html)

    This library contains helpers crates to handle things such as hanging gets.

* [zx](https://fuchsia-docs.firebaseapp.com/rust/zx/index.html)

    Rust language bindings for Zircon kernel syscalls.

* [fdio](https://fuchsia-docs.firebaseapp.com/rust/fdio/index.html)

    Wrapper over fdio library.

* [fuchsia_runtime](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_runtime/index.html)

    Type-safe bindings for Fuchsia-specific `libc` functionality. This crate is
    a minimal extension on top of the `fuchsia-zircon` crate, which provides
    bindings to the Zircon kernel’s syscalls, but does not depend on
    functionality from `libc`.

* [mapped_vmo](https://fuchsia-docs.firebaseapp.com/rust/mapped_vmo/index.html)

    A convenience crate for Zircon VMO objects mapped into memory.

* [mem_util](https://fuchsia-docs.firebaseapp.com/rust/mem_util/index.html)

    Utilities for working with the `fuchsia.mem` FIDL library. This crate is not
    very widely used.

* [shared_buffer](https://fuchsia-docs.firebaseapp.com/rust/shared_buffer/index.html)

    Utilities for safely operating on memory shared between untrusting processes.

* [fidl](https://fuchsia-docs.firebaseapp.com/rust/fidl/index.html)

    Library and runtime for FIDL bindings. For more information about FIDL,
    see [FIDL Overview](/docs/concepts/fidl/overview.md)

* [flyweights](https://fuchsia-docs.firebaseapp.com/rust/flyweights/index.html)

    Types implementing the flyweight pattern for reusing object allocations.

## Packages

* [fuchsia_archive](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_archive/index.html)

    Work with Fuchsia Archives (FARs)

* [fuchsia_pkg](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_pkg/index.html)

    Library that lets you work with Fuchsia packages which are a hierarchical
    collection of files that provides one or more programs, components or
    services to a Fuchsia system.

* [fuchsia-merkle](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_merkle/index.html)

    Protect and verify data blobs using [Merkle Trees](/docs/concepts/security/merkleroot.md)

## Testing

* [diagnostics_log](https://fuchsia-docs.firebaseapp.com/rust/diagnostics_log/index.html)

    Rust interface to the Fuchsia Logging System. This library isn't
    Fuchsia-specific and can be used on the host.

* [diagnostics_reader](https://fuchsia-docs.firebaseapp.com/rust/diagnostics_reader/index.html)

    Utility to let you read metrics and logs. This is useful for creating
    tests.

* [fuchsia_trace](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_trace/index.html)

    A safe Rust interface to Fuchsia's tracing interface.

* [fuchsia_criterion](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_criterion/index.html)

    Thin wrapper crate around the
    [Criterion benchmark suite](https://github.com/bheisler/criterion.rs){: .external}.
    This generates benchmark metrics for infrastructure from criterion benches.

* [fuchsiaperf](https://fuchsia-docs.firebaseapp.com/rust/fuchsiaperf/index.html)

    A library that defines the JSON schema for benchmark metrics.

* [fuchsia_inspect](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_inspect/index.html)

    Components in Fuchsia may expose structured information about themselves
    conforming to the Inspect API. This crate is the core library for writing
    inspect data in Rust components. For a comprehensive guide on how to start
    using `inspect`, please refer to the
    [codelab](/docs//development/diagnostics/inspect/codelab.md).

* [fuchsia_inspect_contrib](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_inspect_contrib/index.html)

    This library is intended for contributions to the inspect library from
    clients.

## Graphics

* [fuchsia-framebuffer](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_framebuffer/index.html)

    Configure, create and use FrameBuffers in Fuchsia.

* [fuchsia-scenic](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_scenic/index.html)

    Rust interface to Scenic, the Fuchsia compositor

## Storage

* [storage](/src/storage/)

    Bindings and protocol for serving filesystems on the Fuchsia platform

    * [vfs](https://fuchsia-docs.firebaseapp.com/rust/vfs/index.html)

    A library to create “pseudo” file systems. These file systems are backed by
    in process callbacks. Examples are: component configuration, debug
    information or statistics.

    * [storage-manager](https://fuchsia-docs.firebaseapp.com/rust/storage_manager/index.html)

    A library to access a file system directory.

    * [fxfs](https://fuchsia-docs.firebaseapp.com/rust/fxfs/index.html)

    A library to us Fxfs which is a log-structured filesystem for Fuchsia.

* [fuchsia-fs](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_fs/index.html)

    Wrapper library for fuchsia.io operations, such as reading and writing files, reading directory
    entries, and watching directories.

## Networking

* [fuchsia_hyper](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_hyper/index.html)

    A library that lets you create a Fuchsia-compatible hyper client for making
    HTTP requests.

* [fuchsia_bluetooth](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_bluetooth/index.html)

    Utilities for Bluetooth development.

## Miscellaneous

* [mundane](https://fuchsia-docs.firebaseapp.com/rust/mundane/index.html)

    A Rust crypto library backed by BoringSSL
