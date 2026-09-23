# Host Model

Implementors of the IP protocol may choose whether to operate as a Weak Host or
a Strong Host, and the decision is referred to as the implementation's
[Host Model](https://en.wikipedia.org/wiki/Host_model). A weak host will accept
a packet received on an interface that is destined for an IP address assigned to
a different interface on the host. A strong host would not accept such a packet.

Strong hosts are generally considered to be more secure, at the cost of less
flexible connectivity.

Netstack3's default behavior is to behave as a strong host; however, it does use
the weak host model in a few situations. This decision is determined on a per
interface basis:

  * If the interface is the loopback device, the netstack will generally
    operate as a Weak Host (see
    [Weak Host on Loopback](#weak-host-on-loopback)).
  * If the interface has unicast forwarding enabled, the netstack will operate
    as a Weak Host (see [Weak Host on Routers](#weak-host-on-routers))

With this decision, Netstack3 prefers to act as a Strong Host for the security
benefits, but is willing to act as a Weak Host in situations that require the
enhanced connectivity.

### Weak Host on Loopback

Netstack3 is a weak host when looping back packets. In particular, it's possible
for a socket bound to an address on one local interface to send to a destination
address that's assigned to a different local interface. The exception to this is
when strict device requirements are in effect, due to:

  1. the sending socket being bound to an interface with `SO_BINDTODEVICE`, or
  2. at least one of the source or destination addresses requires a scope ID.

It is technically possible to permit this, but doing so correctly is difficult,
so this is forbidden for now until a clear need for this arises.

### Weak Host on Routers

Netstack3 is a weak host on interfaces that have IP unicast forwarding enabled,
as they are deemed to be acting as a router. The Weak Host semantics here apply
both to sending and receiving.

On the receiving side, suppose we have two interfaces: `IF-A` and `IF-B`, each
with their own IP address `IP-A` and `IP-B`, respectively. A packet destined to
`IP-B` that is received on `IF-A` will be delivered to the netstack, so long as
`IF-A` has forwarding enabled. The IP layer (and above) will observe that the
packet arrived on `IF-A`. Conceptually we consider the netstack to have
internally forwarded the packet from `IF-A` to `IF-B`. For consistency with this
mental model, the netstack evaluates the packet against the filtering engine's
`FORWARDING` hook. `IF-A` is considered the ingress device and `IF-B` is
considered the egress device.

Now for the sending side, suppose the same setup with the additional knowledge
that the netstack has a route installed via `IF-A` to some external subnet
`SUBNET-C`. A packet originating from the netstack with `IP-B` as its source
address destined for `IP-C` (contained within `SUBNET-C`) will be sent out using
the route via `IF-A`, so long as `IF-B` has forwarding enabled. Once again,
we consider the netstack to have internally forwarded the packet from `IF-B` to
`IF-A` and evaluate the packet against the filtering engine's `FORWARDING` hook.
`IF-B` is considered the ingress device and `IF-A` is considered the egress
device.
