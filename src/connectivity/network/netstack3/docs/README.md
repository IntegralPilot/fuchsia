# Docs

This directory contains documentation of various aspects of the netstack. Briefly:
- [`CONTROL_FLOW.md`](./CONTROL_FLOW.md) describes and motivates how our control flow works
- [`CORE_BINDINGS.md`](./CORE_BINDINGS.md) describes and motivates the split of
  our codebase into two separate "core" and "bindings" components
- [`DETAILS.md`](./DETAILS.md) describes various implementation details that are
  confusing enough to be worth calling out explicitly
- [`DEVELOPMENT.md`](./DEVELOPMENT.md) describes local environment set up for development.
- [`DUAL_STACK_SOCKETS.md`](./DUAL_STACK_SOCKETS.md) describes how dual-stack
sockets are implemented with type safety.
- [`HACKING.md`](./HACKING.md) contains instructions for hacking on the netstack
- [`HOST_MODEL.md`](./HOST_MODEL.md) explains the netstack's host model.
- [`IMPROVEMENTS.md`](./IMPROVEMENTS.md) contains ideas for possible future improvements
- [`IP_TYPES.md`](./IP_TYPES.md) describes our use of types and traits to represent IP versions and addresses
- [`PARSING_SERIALIZATION.md`](./PARSING_SERIALIZATION.md) describes how we
  parse and serialize packets, and how we manage packet buffers
- [`POSIX_COMPATIBILITY.md`](../POSIX_COMPATIBILITY.md) describes our strategy for
  providing POSIX-compatible semantics to applications.
- [`PUB_CRATE.md`](./PUB_CRATE.md) describes how we use visibility modifiers.
- [`STATIC_TYPING.md`](./STATIC_TYPING.md) motivates our enthusiastic use of static typing
