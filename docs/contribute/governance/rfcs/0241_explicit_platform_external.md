<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0241" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at /docs/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

The Fuchsia IDK doesn't specify whether external components should implement
clients or servers for the various FIDL protocols that it defines. This makes it
difficult to correctly evaluate the backwards compatibility implications of
changes to Fuchsia APIs. This RFC proposes a way to express that information in
a both human and machine readable way in FIDL. This unlocks opportunities to
improve the correctness and stability of Fuchsia-based products.

## Motivation

Much of the [Fuchsia System Interface][fuchsia-system-interface] is expressed in
[FIDL][fidl-language] as a set of [protocols][fidl-protocols] and the types that
are exchanged over those protocols. Protocols are asymmetric with a client-end
and a server-end, but the FIDL declarations that are supplied as part of the
SDK/IDK don't specify whether the interface presented by the Fuchsia Platform to
external components provided by products are for the server side, the client
side or both sides of these protocols.

This lack of preciseness significantly constrains the kinds of changes that can
be made safely to the Fuchsia System Interface without knowledge of the
implementation of the Fuchsia Platform, and often external components. In
practice, most protocols will either have only servers or only clients
implemented within the Fuchsia Platform, but this information isn't expressed in
a way that can be checked by tools or discovered by end developers.

For example, if a type that is part of the system interface contains a
`string:100` (a string that takes at most 100 bytes when encoded in UTF-8) and
later we realize that we need to support longer strings. If that type is only
ever sent from external components to platform components (in protocol method
requests) then the length constraint can be safely increased, because the
component decoding a message containing that string will always be at least as
new as the component encoding it. If that type may be sent from platform
components to external components then the length can't be increased safely
because a component built against a newer API level in the platform might
inadvertently send malformed messages to external components.

## Stakeholders

Who has a stake in whether this RFC is accepted?

_Facilitator:_

The person appointed by FEC to shepherd this RFC through the RFC
process.

_Reviewers:_

- abarth@google.com
- chaselatta@google.com
- hjfreyer@google.com

_Consulted:_

- aaronwood@google.com
- awolter@google.com
- crjohns@google.com
- mkember@google.com
- pesk@google.com
- surajmalhotra@google.com
- wittrock@google.com

_Socialization:_

Circulated a 1-pager describing the problem and proposal amongst people working
on platform versioning.

## Requirements

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD",
"SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be
interpreted as described in
[IETF RFC 2119](https://tools.ietf.org/html/rfc2119).

We MUST be able to express which system interface protocols MAY have their
clients or servers or both implemented in platform or external components. This
information MUST be expressed in a way that is accessible to tools that evaluate
platform changes for compatibility.

This information SHOULD be available to tools that run at software assembly time
and MAY be available to the component framework at runtime. This information
SHOULD be made visible to end-developers through generated API documentation and
build-time checks.

For this information to be useful in evaluating compatibility between the
Fuchsia platform and components built upon it, external components MUST be built
against a stable API level that is supported by the platform.

## Design

### Analysis

Compatibility of the platform ABI surface as a whole can be broken down into
considering the compatibility of individual types exchanged between external
components and platform components, both in protocols over channels and
[serialized][fidl-at-rest] and exchanged out of band.

All exchange of FIDL data between components is rooted in one of:

 - FIDL protocols that are exchanged through [component
   framework][component-framework] protocol capabilities. These are marked with
   the `@discoverable` attribute.
 - FIDL protocols that are exchanged through non-FIDL means such as [process
   args][processargs],
 - FIDL types that are [serialized][fidl-at-rest] and exchanged in files (like
   component manifests) or bespoke IPC transports.

Given a view of the whole Fuchsia System Interface FIDL we can, for each root,
collect the set of types that may be sent from clients to servers and from
servers to clients.


#### Protocols

If we know, for each "root" protocol, whether its clients and servers may be
implemented in external components or platform components, we can collect the
set of types that may be sent from external components to platform components,
from platform components to external components, between platform components and
between external components.

For the protocol:

```fidl
@discoverable
protocol {
    M(Req) -> (Resp);
}
```

Given where we allow clients and servers to be implemented we can see where
types in a request (**Req**) vs response (**Resp**) may flow between external
components (**E**) and platform components (**P**):

| Client | Server | E to P    | P to E    | P to P    | E to E    |
| ------ | ------ | --------- | --------- | --------- | --------- |
| E      | E      |           |           |           | Req, Resp |
| E      | P      | Req       | Resp      |           |           |
| E      | P, E   | Req       | Resp      |           | Req, Resp |
| P      | E      | Resp      | Req       |           |           |
| P      | P, E   | Resp      | Req       | Req, Resp |           |
| P, E   | E      | Resp      | Req       |           | Req, Resp |
| P, E   | P      | Req       | Resp      | Req, Resp |           |
| P, E   | P, E   | Req, Resp | Req, Resp | Req, Resp | Req, Resp |

Platform code is guaranteed to speak a superset of the protocol versions spoken
by external code, so we can say the following about whether type constraints can
be tightened or loosened:

| Sender   | Receiver | Constraints   |
| -------- | -------- | ------------- |
| external | platform | can't tighten |
| platform | external | can't loosen  |
| external | external | can't change  |

So we need to be able to constrain where we allow clients and servers to be
implemented.

*Almost* all protocols that are roots are exchanged through component framework
capabilities. Those that aren't are low-level and esoteric. They include
`fuchsia.io.Directory` and a few network socket control channel protocols in
`fuchsia.posix.socket`. Instead of inventing a new way to mark these protocols
as roots of communication we should extend the meaning of `@discoverable` to
incorporate these protocols. In fact there are constants in
`fuchsia.posix.socket` for the names of socket control protocols that will be
automatically generated once they're marked as `@discoverable`.

#### Types

Currently any component (external or platform) code can serialize or deserialize
any non-resource type from raw bytes. To allow compatibility tools to know which
FIDL types _won't_ be serialized or deserialized in some contexts we should
explicitly mark types that are passed between components outside of normal FIDL
IPC.

### Syntax

#### Protocols

The discoverable attribute will be extended to express where discoverable
protocols may be implemented. By default a protocol marked as `@discoverable`
may have clients and servers implemented by both external components and
platform components.

The `server` and `client` optional attributes list the kinds of components that
may implement that kind of endpoint. By default both endpoints can be
implemented by any component.

For example:

```fidl
// All servers in the platform, all clients in external components.
@discoverable(client="external", server="platform")
protocol P {};

// All servers in external components, all clients in the platform.
@discoverable(client="platform", server="external")
protocol Q {};

// Only clients allowed in external components, both clients and servers allowed in the platform.
@discoverable(client="platform,external", server="platform")
protocol R {};

// Servers are only allowed in platform components. Clients are allowed anywhere.
// If both clients and servers are allowed that argument can (and should) be omitted.
@discoverable(server="platform")
protocol S {};
```

#### Types

A new `@serializable` attribute will be added to FIDL to mark which types may be
serialized and passed between components outside of FIDL protocols.

This attribute is only allowed on non-`resource` `struct`s, `table`s and
`union`s.

It has two optional arguments, `read` and `write`. Each of these take a comma
separated list of `platform` and `external` indicating whether platform
components or external components are expected to read or write that type. The
default value of each argument is `"platform,external"` indicating that it can
be read and written from that kind of component.

## Implementation

### FIDL Toolchain
`fidlc` will be updated to accept these the arguments to `@discoverable` and the
new attribute `@serializable`. FIDL bindings generators (`fidlgen_*`) will not
be modified [yet](#fidl-bindings).

### Fuchsia System Interface
All discoverable FIDL protocols in `partner` and `partner_internal` libraries
will be updated. Most will be marked `@discoverable(server="platform")`
indicating that external components should only implement clients but platform
components may implement clients and servers. Some, such as many in `fuchsia.io`
will be marked `@discoverable()` indicating that any component may implement
clients and servers. A few, mostly for drivers will be marked
`@discoverable(client="platform", server="external")` indicating that external
components should only implement servers and platform components should only
implement clients.

After some experimentation and prototyping there doesn't seem to be a
straight-forward way to calculate which category each protocol falls into.
Neither runtime inspection of the component graph, nor static evaluation of CML
shards were clear. Instead we'll look at the manifests of external components
for which protocol capabilities they use and offer.

For standalone data-types we will look for calls to the various language
bindings' explicit serialization APIs and annotate the types that are used there
appropriately.

### Compatibility Tooling
We're developing a tool to evaluate the compatibility of different versions of
the FIDL IPC portions of the Fuchsia System Interface. This works against the
FIDL IR and will incorporate information from discoverable attributes. A
complete description of the compatibility rules that this tool will implement
will come in another RFC.

### Future Opportunities
Compatibility tooling is the motivating priority behind this enhancement to FIDL
syntax. However, this richer information about the Fuchsia System Interface may
prove useful elsewhere.

#### FIDL Bindings
The FIDL bindings generators could become aware of whether they're building
bindings that target platform components or external components and adjust the
code they generate to encourage developers to only implement clients and servers
of protocols in contexts where they have compatibility guarantees.

Preventing any unsupported clients or servers is counter-productive because
developers need to be able to fake or mock their peers in tests, but perhaps we
can place guards to discourage their use in non-test contexts.

The standalone serialization code can be updated to only allow types that are
marked as `@serializable` to be passed in.

#### Component Framework
Within component framework, protocol capabilities are untyped. They're often
named after the FIDL protocol they carry, but that is merely a convention, and
one that is violated. Tools that reason about the protocols that a component
uses to communicate with its peers have to guess, based on the names of
capabilities what protocols are being used.

If the component framework added protocol types to its model these tools would
be simpler, more robust and more correct.

#### Software Assembly
Software assembly produces Fuchsia system images out of configuration, platform
components and external components. It could use information about which
protocols may be served from platform components or external to refuse to
assemble products that violate those rules. This would ensure that product
owners aren't inadvertently building products that make compatibility guarantees
that the Fuchsia isn't offering, and help platform developers to avoid exposing
capabilities in ways that they don't intend to support.

#### Security

Knowing which end of protocol capabilities are routed across the external /
platform boundary may be useful for evaluating the security properties of the
system. This could be done in an ad-hoc fashion, or integrated into existing
tooling.

## Performance

There will be no change to performance.

## Ergonomics

This requires some of the assumptions in the system to be made explicit which
involves a little more work up-front, but can make the system much easier to
understand and work with.

## Backwards Compatibility

Existing FIDL libraries will be unaffected. `@discoverable` without arguments
remains a valid attribute.

Initially we will not want to version the discoverable and serializable
attributes, since they're an input to versioning, have no impact on source or
runtime compatibility, and need to be adopted widely to see benefits. If we
[update FIDL bindings](#fidl-bindings) to changed based on the discoverable
arguments we will have to consider versioning them as changing them could break
existing source code.

## Security considerations

There may be future [opportunities](#security) in this area to improve overall
system security.

## Privacy considerations

None

## Testing

This will make it easier to ensure that we have complete [compatibility
tests][compatibility-tests] for the Fuchsia platform by explicitly encoding
which ends of which protocols we promise compatibility with.

For all of the `fidlc` changes we will add new `fidlc` tests.

Until we are validating at assembly time that all components are following the
rules expressed in FIDL files we won't know for sure that the external /
platform split we've expressed in the SDK matches the reality in Fuchsia
products.

## Documentation

A few pieces of FIDL documentation will need to be updated:

 - the [list of supported attributes][fidl-attributes] will be updated to
   document the changes to `@discoverable` and the addition of `@serializable`
 - the [FIDL rubric][fidl-rubric] will be updated to describe how this should be
   used

## Drawbacks, alternatives, and unknowns

We could keep the status quo, but that would either significantly limit the
kinds of changes we could support, or limit the confidence our tools could have
in checking the safety of changes.

We could express the external / platform split outside of the FIDL language in
the API tooling (like we do for SDK categories), but since this categorization
happens at the declaration level that would feel less ergonomic and more likely
to become stale.

Instead of using `@discoverable` to mark protocols like `fuchsia.io.Directory`,
we could come up with an equivalent attribute that meant the same for
compatibility but didn't include support for protocol capabilities. That felt
like more complexity that was necessary.

Instead of adding `@serializable` to mark types that are exchanged out of band,
we could just write placeholder FIDL `protocol`s that use those typed and are
marked `@discoverable` with appropriate attributes to imply where those types
may be read and written. That would mean having protocols that are never meant
to be spoken and would generally be hard to get right and messy.

Instead of syntax like `@discoverable(server="platform", client="external")`, we
used `@discoverable(platform="server", external="client")` for a little while
but the current syntax feels easier to understand.

## Prior art and references

n/a

[compatibility-tests]: /docs/development/testing/ctf/compatibility_testing.md
[component-framework]: /docs/concepts/components/v2/introduction.md
[fidl-at-rest]: /docs/contribute/governance/rfcs/0120_standalone_use_of_fidl_wire_format.md
[fidl-attributes]: /docs/reference/fidl/language/attributes.md
[fidl-language]: /docs/reference/fidl/language/language.md
[fidl-protocols]: /docs/reference/fidl/language/language.md#protocols
[fidl-rubric]: /docs/development/api/fidl.md
[fuchsia-system-interface]: /docs/concepts/kernel/system.md
[processargs]: /docs/concepts/process/program_loading.md#the_processargs_protocol
