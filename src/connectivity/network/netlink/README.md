# Netlink

Netlink is a socket-based API provided by Linux that user space applications can
use to interact with the Kernel. The API is split into several protocol
families, each offering different functionality. Many of these protocol families
are networking related. See `man netlink` for more details.

This crate implements aspects of Netlink's API surface for Fuchsia: namely it
leaves the socket-oriented functionality as the responsibility of the user, and
focuses instead on the logic of request handling and multicast event
distribution.

## Supported Protocol Families

This crate was authored in a generic way with the intent of supporting multiple
Netlink protocol families. The currently supported families are:
  * `NETLINK_ROUTE`

# Architecture

This crates exposes the `netlink::Netlink` type which represents a single
provider of the Netlink API surface, and provides a means for instantiating new
Netlink clients (aka sockets). Instantiating a new `netlink::Netlink` provides
the type, alongside a Future that *drives the backing Netlink implementation*.

The `netlink::Netlink` type has a generic parameter `P` bound by
`messaging::NetlinkContext`. This provider specifies the type of
`messaging::Sender` and `messaging::Receiver` that will be provided for each
client connecting to a protocol family. A `messaging::Sender` provides a
synchronous API for Netlink to send messages to the client (e.g. the client's
receiving implementation), while a `messaging::Receiver` provides an
asynchronous API for Netlink to receive messages from the client (e.g. the
client's sending implementation).

## Message Parsing

This crate leverages the crates provided by the
[rust-netlink](https://github.com/rust-netlink) open source project to serialize
and deserialize Netlink packets.

## Concurrency Model

Fuchsia's implementation of Netlink is fundamentally asynchronous: most
operations must be dispatched to a separate component behind an asynchronous
FIDL API, whether this be handling an individual request, or waiting for updates
to publish to a multicast group. As such, the implementation is organized into
several `fuchsia_async::Task`s. The tasks can be loosely grouped into the
following categories:
  * Domain-Specific Eventloop : A task responsible for handling all events
    related to a synchronization domain, implemented as an eventloop to allow
    for linearizing events within that domain. Currently, there is one eventloop
    responsible for operations on interfaces, routes, and policy-based-routing
    rules. For routes, this eventloop:
    * keeps track of the current system routing tables by subscribing to the
    `fuchsia.net.routes/WatcherV{4,6}` protocol,
    * publishes events to the `RTNLGRP_IPV{4,6}ROUTE` multicast groups, and
    * handles `RTM_{DEL,GET,NEW}ROUTE` messages by validating the requests, and
    in the case of the `NEW` and `DEL` variants, dispatching the request to the
    `fuchsia.net.routes.admin/RouteSetV{4,6}` protocol.
    This eventloop does similar for interface- and rules-related requests and
    state tracking.
    While routes and interfaces previously were in separate eventloops, this
    incurred complexity for synchronizing between them; as a result, we combined
    them (https://issuetracker.google.com/291629739).

  * Client Request Handler: A task responsible for handling the requests of an
    *individual netlink client* by dispatching them to the appropriate
    domain-specific eventloop and waiting for the result.
  * New Connection Handler: A task responsible for spawning client request
    handlers each time a new Netlink client connects.

A multitask model enables users of this crate to run netlink in a
multithreaded-executor, if they so choose.

## Adding Support for Additional Protocol Families

To add support for `Foo` protocol family, one would need to
1. Define a new `Foo` marker type that implements
    `protocol_family::ProtocolFamily` and
    `multicast_groups::MulticastCapableProtocolFamily`.
2. Define a new `FooRequestHandler` that implements
    `protocol_family::NetlinkFamilyRequestHandler<Foo>`. This may involve
    spinning up additional eventloops backing the implementation of the `Foo`
    protocol.
3. Add a `new_foo_client` method to the `netlink::Netlink` struct. This method
   should instantiate new clients of `Foo`, including spawning a dedicated client request
   handler task.
