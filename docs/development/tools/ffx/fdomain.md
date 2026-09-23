# FDomain overview

[**FDomain**][fdomain] is a mechanism for communicating with FIDL services on a
Fuchsia target device from a development host machine. It was designed to
[replace the Overnet protocol][rfc-228].

Unlike [Overnet][overnet], which is a peer-to-peer mesh network that proxies
kernel handles (often imperfectly), FDomain is a simpler **endpoint-to-endpoint
protocol**. It conceptually represents a collection of handles on the target
that can be manipulated remotely via operations presented over a FIDL protocol.

## Key differences from Overnet {:#key-differences-from-overnet}

- **Endpoint-to-endpoint**: It does not provide facilities for automatic
  discovery or mesh networking.
- **No handles in protocol**: The FDomain protocol itself does not transfer
  kernel handles (the `resource` keyword is never used). Instead, handles are
  referred to by 32-bit IDs allocated by the host or the FDomain.
- **Host-side ID allocation**: To reduce round trips and enable pipelining, the
  host can allocate IDs for new handles it requests to create.
- **Two-way methods**: All methods are two-way and return errors, ensuring that
  unknown method errors are always reported back to the client, improving
  compatibility handling.

## How is FDomain used? {:#how-is-fdomain-used}

FDomain is used to support the functionality of the `ffx` tool and potentially
automated integration tests. It allows a host machine to connect to a Fuchsia
target device and communicate with services via FIDL in much the same way a
component running on the target device would.

FDomain provides operations to:

- Create new sockets, channels, events, and event pairs.
- Close, duplicate, and replace handles.
- Wait for signals on handles.
- Perform reads and writes on channels and sockets.

By presenting an abstraction over Zircon kernel primitives (like channels and
sockets), it allows host-side tools to use FIDL to communicate with the target
device without needing full emulation of the Zircon kernel on the host machine.

## What are Flex Bindings? {:#what-are-flex-bindings}

"Flex bindings" (see the implementation in [`flex.rs`][flex-rs] and related
targets like `flex_fdomain` and `flex_fidl`) provide a conditional abstraction
layer that allows Rust code to be compiled either:

- **With FDomain**: Using FDomain types (like `fdomain_client::Channel`) for
  remote communication from a host machine.
- **With Standard FIDL**: Using standard Fuchsia and Zircon types (like
  `::fidl::Channel`) for local communication on a device.

This is achieved through conditional compilation (for example,
`#[cfg(feature = "fdomain")]`). By using types defined in `flex.rs` (such as
`Dialect`, `AsyncChannel`, and `AsyncSocket`), libraries can be **written once
and used both on-device and driven remotely** via FDomain, without being tied to
a specific transport implementation.

<!-- Reference links -->

[fdomain]: https://fuchsia.dev/reference/fidl/fuchsia.fdomain
[rfc-228]: /docs/contribute/governance/rfcs/0228_fdomain.md
[overnet]: /src/connectivity/overnet/README.md
[flex-rs]: /src/lib/fdomain/client/src/flex.rs
