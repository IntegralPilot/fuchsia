# POSIX-like Compatibility

Fuchsia is [committed][Fuchsia RFC-0184] to supporting a POSIX-like interface
for the system netstack. Like the existing networking stack, Netstack3 will
do this by implementing the same [`fuchsia.posix.socket`] FIDL services.

## How does Netstack3 support a POSIX-like interface?

The Netstack3 ["bindings"][core and bindings] component (a.k.a. the `netstack3`
crate) provides the top-level implementation of the `fuchsia.posix.socket`
services to other Fuchsia components running on the system. The `netstack3`
bindings crate provides a direct implementation of some of these calls, like
`fuchsia.posix.socket.BaseDatagramSocket/GetInfo`, though most calls are
implemented by making calls into `core`.

This requires that `core` provide a call surface that exposes POSIX-like
semantics to bindings, including

- the notion of "unbound" sockets that exist but will not receive traffic,
- the ability to set options on sockets,
- emulation of POSIX `bind()` and `connect()` with address conflict detection,

and more.

Because the `core` surface is written in Rust and called into by other Rust
code, it can provide stronger guarantees than POSIX at compile time via type
constraints. The POSIX [`getpeername`] function, for example, can only be called
on connected sockets.

## Known incompatibilities with existing POSIX-like systems

Netstack3 aims to be POSIX compatible. It also implements a number of common
extensions to the POSIX specification, though it does not emulate or reproduce
the behavior of any particular POSIX-like system.

### `listen(int socket, int backlog)`

The [POSIX specification][POSIX listen] for the `listen` syscall requires that

> If listen() is called with a backlog argument value that is less than 0, the
> function behaves as if it had been called with a backlog argument value of 0.

Linux does not adhere to this requirement, and instead treats a backlog size
less than 0 as requesting the maximum. Netstack3 treats a backlog size of 0 or
less as requesting the minimum, and applies a minimum backlog size of 1.

### `SO_REUSEPORT`

Netstack3 supports the `SO_REUSEPORT` socket option present in Linux and
BSD-based systems. The behavior of sockets that have `SO_REUSEPORT` set is
similar on Netstack3 and Linux with regards to sending and receiving packets.
Where Linux allows setting a socket's `SO_REUSEPORT` flag at any point,
including after it has been bound, Netstack3 only allows setting `SO_REUSEPORT`
on a socket before it is bound to a local address.

TODO(https://fxbug.dev/411702408): This socket option is only supported for UDP
sockets and not TCP sockets.

### `SO_BINDTODEVICE`

Netstack3 supports setting the interface on which a socket will send and receive
packets using the Linux `SO_BINDTODEVICE` socket option. Unlike Linux, Netstack3
checks that the bound device does not conflict with other functionality that
controls the interface used for sending and receiving, including
- [`IPV6_MULTICAST_IF`] and [`IP_MULTICAST_IF`],
- for an IPv6 socket, the scope ID associated with its local or remote address,

### `IP_MULTICAST_IF`

On Linux the IPv4 address set with `IP_MULTICAST_IF` is also used as the source
address for outgoing multicast packets. Netstack3 uses the regular source
address selection algorithm, using the provided address only to pick the
multicast interface.

### `SO_SNDBUF` and `SO_RCVBUF`

Netstack3 supports the [`SO_SNDBUF` and `SO_RCVBUF`][POSIX buffer sizes] socket
options for setting a socket's send and receive buffer sizes, respectively. On
Linux, the value provided when setting one of these options
[is doubled][Linux buffer sizes], so reading the value for the same option
returns a different value. Netstack3 handles setting one of these buffer sizes
by using the value to set the variable-size portion of a socket's buffer.
Reading the same buffer size value reports the sum of the variable and any
fixed-size portion of the socket's buffer.

Note that like on other platforms, the value applied when setting these options
is limited by system-defined minimums and maximums.

### Dualstack operations on ICMP Echo sockets

ICMP echo sockets do not support dual stack operations. Despite this, Linux
still allows one to set/get various dualstack socket options on Ipv6 ICMP
sockets, though doing so does not affect the socket's behavior in anyway.
Netstack3 has opted to disallow setting/getting these options on IPV6 ICMP
sockets, to more accurately reflect that dualstack operations are not supported.
These socket options include
  * `IPV6_V6ONLY`: Attempting to set the value will result in `ENOPROTOOPT`,
    while getting the value unconditionally returns true.
  * `SO_IP_TTL` & `SO_IP_MULTICAST_TTL`: Attempting to set or get the value will
    result in `ENOPROTOOPT`.

### UDP Destination Port 0
Like Linux, Netstack3 allows UDP sockets to connect to a remote address with
port 0. Calling [`getpeername`] on such a socket results in `ENOTCONN`. However
unlike Linux, Netstack3 disallows:

  1. Sending packets to the remote. Calling `send` on the socket results in
    `EDESTADDRREQ`.
  2. Receiving packets from the remote. Packets received whose source port is 0
    are be dropped as malformed.

On Linux, calling `send` on the socket is expected to succeed and generate a
packet on the wire whose destination port is 0. When receiving traffic, Linux
treats a destination port of 0 as a wildcard, delivering packets to the socket
regardless of the packet's source port (note that the packet's source address
must still match the socket's remote address).

### Dualstack UDP connect/send-to mismatched IP version

Netstack3 enforces that the peer address has the same IP version as the local
address while sending. For example on a IPv6 UDP socket:
  1. Binding to an IPv4-mapped-IPv6 address then connecting or sending to an
  IPv6 address will result in EAFNOSUPPORT.
  2. Binding to an IPv6 address then connecting or sending to an
  IPv4-mapped-IPv6 address will result in ENETUNREACH.
On Linux, both of these operations are supported (with slightly different
semantics). The first case will result in the packet being sent to the IPv6
destination with a system-selected IPv6 source address. The second case will
result in the packet being sent to the IPv4-mapped-IPv6 destination *as an IPv6
address* with the bound IPv6 source address.

### TCP bind to multicast/broadcast IP addresses

On TCP sockets, Netstack3 returns EADDRNOTAVAIL if `bind` is called with a
multicast or broadcast address as TCP is a unicast-only protocol. On Linux,
this does not produce an error, though the resulting socket will not receive
packets; this is because TCP implementations are
[required][TCP RFC 9293: Source Address Validation] to drop all
incoming multicast or broadcast SYN packets. Netstack3 opts to provide earlier
notification to the user that the requested operation is invalid for TCP.

[Fuchsia RFC-0184]: /docs/contribute/governance/rfcs/0184_posix_compatibility_for_the_system_netstack
[`fuchsia.posix.socket`]: /sdk/fidl/fuchsia.posix.socket/socket.fidl
[core and bindings]: ./CORE_BINDINGS.md#core-and-bindings
[`getpeername`]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/getpeername.html
[POSIX listen]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/listen.html
[`IPV6_MULTICAST_IF`]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/V2_chap02.html
[`IP_MULTICAST_IF`]: https://man7.org/linux/man-pages/man7/ip.7.html
[POSIX buffer sizes]: https://pubs.opengroup.org/onlinepubs/9699919799/functions/V2_chap02.html#tagtcjh_8
[Linux buffer sizes]: https://man7.org/linux/man-pages/man7/socket.7.html
[TCP RFC 9293: Source Address Validation]: https://datatracker.ietf.org/doc/html/rfc9293#section-3.9.2.3-2.2
