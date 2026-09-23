# loopback_resolver

The loopback resolver is a trivial [component resolver](resolver) that proxies
a `Resolver` request to its incoming `fuchsia.component.resolution/Resolver`
protocol.

This is useful if you want to transform a
`fuchsia.component.resolution/Resolver` [`protocol`](capability-protocol)
capability into a [`resolver`](capability-resolver) capability. It can also be
used to break a cycle that would occur if you assigned a `resolver` to a child
environment directly, by adding the `loopback_resolver` as a child component
and feeding it the `fuchsia.component.resolution/Resolver` which was offered as
`weak` by the parent. However, this should be done with caution since once the
source resolve is shutdown (during system shutdown, for instance) further
attempts to resolve components backed by the `loopback_resolver` will fail.
