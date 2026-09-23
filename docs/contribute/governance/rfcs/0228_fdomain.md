<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0228" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This proposal outlines FDomain, a new mechanism for communicating with FIDL
services on a Fuchsia target from a development host. FDomain will support the
functionality of the FFX command line tool, replacing the Overnet protocol used
today.

## Motivation

FFX offers more or less arbitrary access to FIDL protocols exposed in the
component hierarchy. This has made it easy to develop a wide range of tools and
plugins, and shows great potential to allow automated integration testing and
functionality similar to sl4f.

FFX communicates with on-device FIDL via Overnet.
[Overnet](/src/connectivity/overnet/README.md) is a peer-to-peer protocol that
allows Fuchsia kernel handles to be shared over the network. A Fuchsia device
can send a channel via overnet to a developer host, and a host side Fuchsia
emulation library will allow that channel to be used much as it could on the
device itself. Rust's FIDL bindings can compile on host using these channels and
so FIDL can be used to communicate from host to target.

There are several drawbacks to Overnet's approach:

* The emulation of kernel handles on the host is necessarily imperfect. Handles
  are confined to a single process and additional Overnet connections are
  necessary to move them between processes on the host. Certain mechanisms, such
  as zx_channel_call, are missing entirely.
* Error reporting from handles was not designed around the idea that they were
  being supported by a complex and fallible network stack. Any
  service-interrupting issue that occurs with an Overnet handle ends up being
  reported as PEER_CLOSED with no further information.
* Kernel handles were not designed to be proxied over the network. Overnet was
  designed and implemented without sufficient consultation of the kernel team.
  As a result, correct implementation of some features, such as object signals,
  is impossible with current APIs and likely to remain impossible.
* The commitment to proxying handles and having them behave more or less the
  same remotely on the host as they would locally on the target makes supporting
  certain objects, such as VMOs, almost completely impossible.

In addition to the problems with the approach, Overnet's specific design and
implementation also has significant drawbacks:

* Overnet's low-level protocols, which it uses to coordinate streams, are
  written with FIDL, but do not use FIDL in the way it was designed. Rather,
  FIDL serves in a protobuf-like role, specifying the binary shapes of packets
  but having no role in transport. This has left Overnet vulnerable to wire
  format changes in FIDL in ways that the FIDL team could not manage as it could
  for FIDL users operating over normal transports.
* Overnet's code is overly-complex and uses asynchronous patterns that have
  proven to be extremely difficult to debug.
* Overnet was designed as a peer-to-peer mesh network, an additional layer of
  complexity that is not needed for any present-day use case.


## Stakeholders

_Facilitator:_

hjfreyer@

_Reviewers:_

abarth@
cpu@
ianloic@
mgnb@
mkember@
slgrady@
wilkinsonclay@

_Socialization:_

The design of FDomain was discussed within the Fuchsia tools team, and a basic
proposal was shown to the kernel and FIDL teams as well, both of whom offered
feedback.

## Requirements

FDomain should allow a host to connect to a Fuchsia target and, where
configured, communicate with services via FIDL in much the same way a component
on the target would. As with Overnet, this will be implemented by supporting
communication with lower-level kernel primitives (channels specifically) and
FIDL bindings can then be implemented on top of that communication. Overnet
currently supports proxying channels, sockets, events and event pairs, and thus
can be used to communicate with any FIDL protocol that does not transfer handles
outside of those types. FDomain will be specified here as supporting that same
set of handle types, but must have a clear path to be extended to support a
maximal set of Fuchsia handle types, with the hope being that all handle types
can be supported. VMOs are of particular interest for several applications and
will likely be an immediate area of future work. Note that not all operations on
all handle types make sense to present in a remote context (e.g. mapping and
unmapping VMOs remotely is not useful), but FDomain seeks to present as much
capability with respect to each handle type as possible.

Related to the above, FDomain should present an abstraction over the wire that
allows similar capabilities to directly using the kernel interfaces, but in a
way that does not break the assumptions on which those interfaces were designed.
Many of the limitations of Overnet are due to its transfer-and-proxy model of
presenting handles to the host being incompatible with handle types outside of
its original supported set.

FDomain's design will assume the ability to exchange reliable, ordered datagrams
between the target and the host. This should be trivial to implement on top of a
TCP or SSH connection with a small additional protocol layer, or on top of a USB
bulk endpoint. Specifics of these transports is beyond the scope of the core
protocol.

Unlike Overnet, FDomain will not provide any facilities to discover or establish
connections to targets automatically. FFX already does not rely on Overnet's
implementations of these functions. FDomain, unlike Overnet, is purely an
endpoint-to-endpoint protocol, not a mesh networking protocol.

FDomain should be able to detect protocol version incompatibilities such that
tools can surface issues arising therefrom easily. It is difficult to foresee
what sort of compatibility issues could be introduced in the future, but in
general, FDomain should interoperate as well as possible with incompatible
versions. Where FDomain uses FIDL directly, it must use facilities maintainable
by the FIDL team themselves to ensure wire format compatibility and fall back
where available.

### Requirements for the user API

While the FDomain protocol and the semantics it exposes directly are important
to design correctly, there are also important constraints on the APIs presented
to the user directly. These will take the form of a host-side Rust crate which
provides functions for connecting to an FDomain from a host.

While FDomain deliberately does not present an exact emulation of Zircon, it
also must support bindings which do not disrupt the "programming model" of
Zircon itself. That is, working with handles in an FDomain from the host  should
not have subtle or surprising differences with working with handles from Zircon
code.

Determining exactly what differences are safe to allow between the Zircon and
FDomain host-side APIs is a subject for API review, beyond the scope of this
RFC. However we will note a few expected differences that will be necessary for
FDomain to achieve its goals:

*  FDomain actions on handles will usually require IO, so we cannot implement
   the non-blocking semantics of the Zircon APIs. The Rust bindings for Zircon
   handles include higher-level "async" versions of each handle which handle
   asynchronous operations using Rust's futures system. We will likely implement
   the FDomain host-side library at that layer, without providing the lower
   level handles that provide direct access to non-blocking operations. This
   will result in some semantic differences around object creation. Also,
   operations which normally took only the raw handle, not the async version,
   such as creating endpoints in the FIDL APIs, will not be able to make this
   distinction with FDomain.
*  FDomain handles are necessarily part of a remotely-connected FDomain.
   The RAII objects representing them in our API will be produced from a
   connection object, not constructed in place, and may have behaviors or
   type properties which reflect the fact that their lifecycle is tied to the
   lifecycle of the connection. (Since some debate has occurred about this
   portion of the design, we will emphasize that this RFC is making no
   *specific* recommendation here).
*  FDomain operations will return a different, richer error type reflecting the
   more complicated failure modes a remote protocol can experience (and the
   richer error reporting we can afford in a remote context). Rust's programing
   conventions around error handling should make this change fairly easy for
   users to navigate.

## Design

Conceptually, an FDomain is a collection of handles which can be manipulated
remotely via a set of operations. The FDomain protocol presents these operations
via a FIDL protocol.

The FDomain is connected to, and manipulated by, a host, and the connection
between FDomain and host is an FDomain connection. An FDomain only ever has at
most one connected host, meaning a host should never observe the handles in the
FDomain it controls being closed or written to or read from by another actor.
While session resumption might be an interesting area of future work, we assume
for this document that a host always connects to a new FDomain, which is a
pre-populated collection of handles, and that when the host disconnects the
FDomain is destroyed and the handles within it closed.

### Core Protocol

Since the point of FDomain is to present FIDL over a connection that does not
support transfer of kernel handles, and usually to a host which does not support
them either, The FDomain FIDL itself cannot use handles anywhere in its method
parameters or return values. Put specifically, the `resource` keyword must never
appear in FDomain's FIDL specification.

Because FDomain's protocol messages are designed to be transported over media
other than channels, we assume no message size limit, and accordingly do not
limit sizes of vectors and other structures within the protocol.

#### Error Reporting is Mandatory

All methods must be capable of returning an error. Since our error type is
extensible, this allows us to add error conditions to all methods in the future.

In addition, this forces all methods to be two-way methods, which means that
unknown interaction handling as specified in (RFC-0138)[RFC-0138] will always be
able to return an error to the host about an unknown ordinal. This should make
our backwards compatibility stories easier to implement, as per the "smart
sender, dim receiver" principal, all compatibility errors will surface on the
host.

#### Handle ID Allocation

Each handle within an FDomain will be referred to by the host via an ID. These
IDs are encoded in the protocol as 32-bit unsigned integers, but SHOULD NOT be
the actual kernel handle number directly exposed to the host.

In order to reduce round trips necessary to perform operations in the protocol,
we will be using host-side ID allocation where possible. This means that if an
operation would cause a new handle to be produced within the FDomain, such as
when the host requests the creation of a new channel or socket, the host
provides the ID numbers which the new channel will use themselves. This allows
better pipelining in the protocol, as the host can submit the request that the
handle be created along with the first operations on that handle in the same
transaction, without having to wait for a reply for each operation.

The exception to this policy is reads from channels, which may produce an
arbitrary number of handles. Requiring the host to allocate IDs for handles
which may be produced by reading a channel makes submitting read requests
unwieldy and requires specifying complicated blocking semantics in order to be
compatible with streaming reads. As such handles produced by reading channels
will be assigned IDs by the FDomain itself.

#### Bootstrapping

When a host connects to an FDomain, it is assumed from context to be populated
with a few starting handles. These handles resemble the handles that would be
provided to a newly-started component. The FDomain will allow the host to
retrieve these handles in a way that supports that analogy, preserving a user
intuition where an FDomain is a node in the component topology.

#### Handle Operations

FDomain will support creating new sockets, channels, event pairs, and events
inside of the FDomain. It will also support closing and duplicating handles, and
replacing a handle with a new version with different rights, similarly to
`zx_handle_replace`.

For reading, writing, and other operations that may have to be retried (i.e. may
fail with `ZX_ERR_SHOULD_WAIT`), the FDomain protocol will implement a hanging
get and perform the necessary port waiting operations on the target side. This
will avoid the clumsiness and large number of round trips involved in requiring
the host to set up a port manually.

### Stable Wire Encoding

FIDL's transaction format was not designed to be transmitted outside of Fuchsia
channels. When FIDL needs to be serialized in other contexts, the persistent
headers are recommended, as outlined in RFC-0120.

The usual use of the persistent header is for FIDL data which will be stored at
rest. While FDomain messages won't be transmitted over channels as is usually
the case, they are still meant to be sent and received in the usual
request-response fashion.

The persistent header and the transaction header both contain a magic number and
a series of compatibility flags. The transaction header also adds a "dynamic
flags" field, used for the new flexible methods protocol, and a transaction ID
and method ordinal, both of which we will require for FDomain messages.

Initially, the plan was to use the persistent header and add the transaction ID
and method ordinal manually creating a new combined header format. All of
FDomain's methods are flexible so the dynamic flag field's value could be
implicit.

On review, the FIDL team noticed that the combined header was so similar to the
transaction header that they proposed simply using the normal FIDL transaction
header. This RFC will adopt that proposal, with the understanding that the FIDL
maintainers accept any ways this might change the evolutionary pressures on the
transaction header format.

## Implementation

Target-side FDomain support can be provided in a single Rust crate, with the
first integrator likely being the Remote Control Service.

A transport to the device may be provided first as a socket distributed over the
legacy Overnet protocol, or as an additional circuit within the lower layer of
said protocol. From there, we may choose to implement a direct transport over
SSH, or to move directly to implementing a bespoke transport.

From the target side more work is needed. Current host-side FIDL communication
over Overnet relies on an emulation of Fuchsia's kernel primitives which lives
in the fuchsia-async library. FDomain is explicitly designed for use from
non-Fuchsia targets and should not rely on this emulation. To allow this, we
will require a new kind of Rust FIDL binding for FDomain users that works on top
of an FDomain set of primitives. These primitives will represent individual
handles in the FDomain, but will not attempt to perfectly emulate the Fuchsia
kernel interface. It is in fact desirable that these handles be explicit about
the fact that they are handles to remote objects, as their APIs can expose
errors related to the unique failure conditions associated with that scenario.
Contrast Overnet, where any failure of the transport or internal protocol
implementation results in an undifferentiated `ZX_ERR_PEER_CLOSED` for the user.

### Migration

An FDomain target-side implementation will depend on the Zircon APIs for
manipulating handles. Because Overnet provides these APIs, it should be possible
to run an FDomain on top of an Overnet node.

This will be our initial strategy for migration within FFX. We will host an
FDomain in the FFX daemon, which obtains an RCS proxy from the legacy Overnet
connection, and exposes it in its namespace. Hosts can use a magic initiator to
request a new socket connection become either a normal Overnet connection or an
FDomain connection (currently overnet connections start with the magic string
"CIRCUIT\0", so we already look for magic in this position).

From here the FFX tool framework will be able to provide either Overnet or
FDomain proxies to tools, possibly a mix of both if two connections are
established. This should allow us to migrate individual tools easily to the
FDomain host-side code. Services hosted in the daemon itself are inconvenient to
use from FDomain, but other architectural pressures are already encouraging such
services to go away or be rethought.

Since FFX plugins only consume FIDL proxies in general, we should be able to
migrate to an FDomain connection hosted directly by the target with no further
changes to them. We should be able to support both connection atop Overnet and
direct FDomain connection in parallel, as we did with the circuit-switched
Overnet migration.

## Performance

FDomain's existing applications are all developer tools currently handled by
Overnet. There has never been a serious effort to benchmark Overnet performance
as developer tools usually don't have strict latency or large bandwidth
requirements.

That said, questions about Overnet's performance have started to arise, and
at least establishing what performance expectations are is likely worthwhile. As
such a cursory effort should be made to test the following:

* Throughput communicating with a socket, blocking and streaming.
* Latency when making FIDL calls directly to a simple interface.
* Latency when making FIDL calls repeatedly where each call is made on a channel
  sent in reply to the previous call. This will establish how handle creation
  operations show up in performance-sensitive application.

## Backwards Compatibility

FDomain will represent a clean break from the Overnet protocol. Backward
compatibility will be preserved by maintaining both protocols for some grace
period before dismantling Overnet entirely.

## Security considerations

The FDomain core protocol does not handle authentication directly and is
designed to allow arbitrary access to the collection of handles it controls. It
should be deployed in such a way as to give access only to handles which can be
safely shared with the accessor, and in environments where it is safe to give
such access. These considerations were already taken for Overnet, so taking over
its applications should ensure these things.

In future applications where privilege escalation is more of a concern (Overnet,
and thus whatever replaces it, is by nature a maximally-privileged application),
one could envision a class of attack where an attacker manipulates handles with
FDomain that are not actually intended to be in the FDomain. Following the
guideline in this document of not using actual kernel handle numbers as handle
IDs in the FDomain protocol should mitigate all obvious vectors for such an
attack. Implementing FDomain in Rust will also make these attacks easier to
defend due to the language's strong ownership semantics.

## Privacy considerations

FDomain is intended for use on devices under development. It should not have
unique privacy concerns. Even in unanticipated applications it is unlikely to
have unique privacy concerns except those which follow from security concerns.

## Testing

We've had success with Overnet in producing integration tests hosted within a
single process, where multiple instances of the protocol's state are connected
to each other simply by providing the bytes output from one instance to the
input of another. This should let us test a variety of configurations and
topologies.

Integration tests involving real, networked components are more difficult, but
may not add much additional value and can introduce the flake risks inherent in
a complicated physical test setup. We will use facilities to do such testing
where the infrastructure is readily available, and expand that testing as
availability changes.

## Documentation

Apart from documenting the implementation deliverables in the usual manner, a
living form of the protocol specification should exist in the tree. This will
consist largely of the text of the [Design](#design) section of this RFC at
first but can adapt to incorporate protocol extensions as needed.

## Drawbacks, alternatives, and unknowns

FDomain represents a considerable reduction in complexity compared to Overnet.
It also must be implemented from scratch and has no backward compatibility with
Overnet.

The space of alternative designs for a remote tools protocol is vast, so
enumerating alternatives given the assumption a new protocol is needed is a long
and unproductive task. We could have chosen a custom binary format over FIDL,
thereby potentially gaining more control over our compatibility story. Or we
could have presented an IPC mechanism that did not represent kernel objects at
all but instead provided specific bespoke interfaces to tooling.

If Fuchsia's use cases expand it is likely other uses for FDomain will surface
out of a desire to duplicate less work and offer fewer protocols. One could
envision Fuchsia servers in a data center with FDomain replacing SSH as a remote
administration transport rather than simply a diagnostic tool. The FDomain
protocol described here should hopefully be useful in general for such things,
but it is impossible to know for certain how it would scale to such tasks.

## Prior art and references

Please see the [Overnet docs](/src/connectivity/overnet/docs) in the Fuchsia
tree for more information on that protocol.

<!-- xrefs -->

[RFC-0138]: /docs/contribute/governance/rfcs/0138_handling_unknown_interactions.md
