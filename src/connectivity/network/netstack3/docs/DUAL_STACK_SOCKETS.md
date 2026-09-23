# Dual-stack sockets

The Netstack3 core crate exposes IPv4 and IPv6 UDP and TCP sockets to bindings,
which uses them to provide a POSIX-compatible socket API. For IPv6 sockets, that
means supporting communication with IPv6 hosts, and also IPv4 hosts via
[IPv4-mapped IPv6] addressing.

## What does "dual-stack" mean?

IETF [RFC 3493] specifies how the POSIX socket API was extended to support
IPv6 sockets. IPv6 sockets that do not have the `IPV6_V6ONLY` option enabled
have "dual-stack" behavior: they can be used to communicate with IPv4 hosts or
with IPv6 hosts.

Not all network socket protocols are capable of dual-stack behavior. Netstack3
supports dual-stack operation only for TCP and UDP, but not for ICMP or raw IP
sockets.

## How dual-stack behavior is implemented

Netstack3 core exposes socket functionality to bindings via functions that are
templated over the IP version. These are implemented internally by dispatching
to handler and context traits that are implemented generically for both IP
versions. Even though only IPv6 sockets are capable of dual-stack operation,
preserving the IP-generic implementation of socket protocol functionality is
useful for reducing code duplication.

To implement behavior generically, we move the question of whether an
`[IP version, socket type]` supports dual-stack operation into the type system.
That lets us use Rust's exhaustive enum matching to ensure that all possible
cases are handled. There are a couple techniques that make this possible:

1. Exposing the variability of dual-stack/non-dual-stack support as an enum from
   a `dual_stack_context` method provided by a context trait. This allows us to
   write IP-generic socket handler code that matches on the result of the method
   and handles both cases.

2. Provide dual-stack-specific and non-dual-stack-specific behavior behind
   traits. Instances of types that implement these traits are returned in the
   enum variants from the `dual_stack_context` method. This means that socket
   handler code has access to additional functionality in the `match` branches
   for dual-stack-capable and non-dual-stack-capable sockets.

3. Use uninstantiable types for associated types that will never be constructed.
   The `dual_stack_context` trait method from above returns a notional
   `Either<Self::DualStackContext, Self::NotDualStackContext>` which can contain
   an instance of one or the other associated types. For a given
   `[IP version, socket type]`, the choice is fixed and knowable at compile time
   - only one of these types is ever instantiated, so the other can be
   uninstantiable! That allows the compiler to optimize out branches in
   monomorphized code, making the abstraction free in terms of performance. It's
   also [trivial][UnreachableExt] to implement trait methods on uninstantiable
   types (assuming the trait methods take `self`, `&self` or `&mut self`).

Putting these together, we get something like the following:
```
use core::convert::Infallible as Never;

trait StateContext<I: IpExt> {
   type DualStackContext: DualStackStateContext<I>;
   type SingleStackContext: SingleStackStateContext<I>;
   fn dual_stack_context(&mut self)
      -> Either<&mut Self::DualStackContext, &mut Self::SingleStackContext>;
}

trait DualStackStateContext<I: IpExt> { /* ... */ }
trait SingleStackStateContext<I: IpExt> { /* ... */ }

impl StateContext<Ipv4> for &SyncCtx {
   type DualStackContext = Never; // uninstantiable
   type SingleStackContext = Self;
   fn dual_stack_context(&mut self)
      -> Either<&mut Self::DualStackContext, &mut Self::SingleStackContext> {
         Either::Right(self)
      }
}

impl StateContext<Ipv6> for &SyncCtx {
   type DualStackContext = Self;
   type SingleStackContext = Never; // uninstantiable
   fn dual_stack_context(&mut self)
      -> Either<&mut Self::DualStackContext, &mut Self::SingleStackContext> {
         Either::Left(self)
      }
}

// Real implementations that access state.
impl DualStackStateContext<Ipv6> for &SyncCtx { /* ... */ }
impl SingleStackStateContext<Ipv4> for &SyncCtx { /* ... */ }

// Implementations on uninstantiable types with unreachable method impls.
impl<I: IpExt> DualStackStateContext<I> for Never { /* ... */ }
impl<I: IpExt> SingleStackStateContext<I> for Never { /* ... */ }
```

## Holding dual-stack state

Some socket state types are different for a particular dual-stack-capable
protocol (UDP or TCP) depending on the IP version. For example, the state for a
bound IPv4 socket should hold only an `Option<Specified<Ipv4Addr>>` as the bound
address. An IPv6 socket, however, can be bound to either an assigned IPv6
address or an IPv4-mapped IPv6 address (or to the unspecified address), and the
type for storing the address should reflect the additional possibility.

The tricky part is that this bound address will comprise one field in an
IP-generic "bound state" type. We can model the IP variability by declaring an
associated type on an IP extension trait and using that in IP-generic code. So a
bound socket state might look like this:
```
struct BoundSocketState<I: IpExt> {
  bound_addr: I::BoundAddr
}

trait IpExt: Ip {
  type BoundAddr;
}

impl IpExt for Ipv4 { type BoundAddr = Option<Specified<Ipv4Addr>>; }
impl IpExt for Ipv6 { type BoundAddr = DualStackBoundAddr<Ipv6>; }
```

This allows us to store the state but not to easily access it, since IP-generic
code sees only `I::BoundAddr`, not the possible concrete types. To enable access
to the concrete types, we provide IP-generic code access to infallible
conversion routines via the same dual-stack and non-dual-stack context traits
from earlier. This lets IP-generic code convert IP-generic types like
`I::BoundAddr` into their dual-stack or non-dual-stack versions. Since the
conversions are implemented on the same types returned by the
`dual_stack_context` trait method, it's only possible to perform a conversion
once the question of dual-stack or non-dual-stack operation is already known.

Extending the above example:
```
trait DualStackContext<I: IpExt> {
   fn to_dual_stack_bound_addr(&self, addr: I::BoundAddr)
      -> DualStackBoundAddr<I>;
}

trait SingleStackContext<I: IpExt> {
   fn to_single_stack_bound_addr(&self, addr: I::BoundAddr)
      -> Option<Specified<I::Addr>>;
}
```

These conversion routes are trivially implementable for uninstantiable types
(see above) and should be effectively pass-through methods on concrete
dual-stack and non-dual-stack context trait implementations:

```
impl DualStackContext<Ipv6> for &SyncCtx {
   fn to_dual_stack_bound_addr(&self, addr: Ipv6::BoundAddr)
      -> DualStackBoundAddr<Ipv6> {
      // Ipv6::BoundAddr is definitionally DualStackBoundAddr<Ipv6>.
      addr
   }
}

impl SingleStackContext<Ipv4> for &SyncCtx {
   fn to_single_stack_bound_addr(&self, addr: Ipv4::BoundAddr)
     -> Option<SpecifiedAddr<Ipv4Addr>> {
      // Like above, Ipv4::BoundAddr is definitionally the output type.
      addr
   }
}
```

[IPv4-mapped IPv6]: https://en.wikipedia.org/wiki/IPv6#IPv4-mapped_IPv6_addresses
[RFC 3493]: https://datatracker.ietf.org/doc/html/rfc3493
[`core::convert::Infallible`]: https://doc.rust-lang.org/std/convert/enum.Infallible.html
[UnreachableExt]: https://fuchsia-docs.firebaseapp.com/rust/explicit/trait.UnreachableExt.html
