<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0235" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This RFC proposes runtime and [declarative][docs-component-manifests] APIs for
creating and routing bundles of capabilities, called *dictionaries*.

## Motivation

Today, the component framework only supports "point to point" routing for
capabilities it defines: in order to route a capability C from component A to
component B, a route segment must exist in every intermediate component to route
C between the adjacent components.

A multitude of use cases that would strongly benefit from the ability to route a
bundle of capabilities as a single logical unit. Without this feature, these
customers have to resort to workarounds that are costly, inflexible, and
brittle. Here is a sampling of these use cases:

-   Diagnostics capabilities such as `LogSink`, `InspectSink`, and trace
    provider are consumed by almost every component and it would simplify the
    routing topology significantly if we could route them as a bundle.
-   Profiler capabilities such as `fuchsia.debugdata.Publisher` are in a similar
    category as diagnostics capabilities, except they are only enabled on
    certain builds. Currently we don't have a good way for builds to configure
    the addition of a capability that needs to span the entire topology. This is
    the reason profilers are only enabled in test realms today.
-   Test realm proxies expose an interface to a test case that the test uses to
    exercise the components under test. Right now, this is a custom interface
    that bears much resemblance to the component connection APIs. This interface
    could be replaced with a bundle of capabilities that the test realm proxy
    transfers to the test. This way the test realm proxy would not have to
    define its own abstraction layer and could make any component framework
    feature available for the test to use.
-   Anywhere capabilities are routed through multiple layers, the routing could
    be simplified with bundling:
    -   `session_manager`: All the capabilities routed from `core` to
        `session_manager` need to be re-routed by `session_manager.cml`. This is
        a large set of capabilities so it's hard to maintain, and it means some
        non-platform capabilities leak into `session_manager.cml`.
    -   `chromium`: Chromium cml files contain a lot of duplication and would be
        greatly simplified if capabilities could be grouped and routed under a
        single name. [https://fxbug.dev/42072339](https://fxbug.dev/42072339))
-   Component Framework [environments][docs-component-environments] are a
    feature whereby [runners][docs-component-runners] and
    [resolvers][docs-component-resolvers] can be configured to be made available
    to an entire subtree and implicitly routed. If we could use a common
    bundling API to accomplish this instead, that would be more harmonious with
    the rest of the Component Framework routing API and alleviate
    least-privilege concerns with environment-based implicit routing.

The following are also motivating use cases for capability bundling, but they
have some special considerations that will require followup design work beyond
the proposal in this RFC.

-   Some board-specific drivers would like to expose custom services that aren't
    defined for the platform. These services should be exposed by the
    `bootstrap` realm because all driver components live there, but it doesn't
    make sense to explicitly name these components in the platform topology. We
    could deal with this if cml had a way to bundle these services together.
-   A similar problem arises in tests that use `driver_test_realm`. These tests
    wish to route different services from the driver to the test. In these tests
    the driver test realm component is sitting between the drivers and the test,
    and we would like to be able to reuse the driver test realm in these tests
    without modifying it.

Finally, there are already several existing component framework APIs that
involve grouping capabilities. However, they are independent and only apply to
particular situations. Here are a few examples:

-   The [namespace][docs-process-namespace] is a grouping of all the
    capabilities routed to a program (those in its `use` declarations).
-   The [exposed directory][fidl-component-exposed-dir] is a grouping of the
    capabilities a component routes to its parent, in other words, its public
    interface.
-   A [service capability][docs-component-services] is a way of grouping
    protocols together, and service capabilities themselves can be grouped
    together to form "aggregated" service capabilities.

The fact that we have so many APIs hints that users would benefit from having
general abstraction that allows them to define their own dictionaries and route
them.

## Stakeholders

The following teams have been identified as stakeholders, based on the use cases
listed above:

-   Architecture
-   Diagnostics
-   Testing
-   Toolchain
-   Driver Framework
-   Security

*Facilitator:* hjfreyer@

*Reviewers:*

-   abarth@ (Architecture)
-   crjohns@ (Testing)
-   markdittmer@ (Security)
-   miguelfrde@ (Diagnostics)
-   surajmalhotra@ (Drivers)
-   ypomortsev@ (Component Framework)

*Consulted:*

-   phosek@
-   kjharland@
-   anmittal@
-   wittrock@
-   novinc@

*Socialization:*

Two internal documents preceded this RFC: a use cases and requirements doc, and
a core design doc. These documents have received informal approval from
stakeholders.

Information from these documents has been incorporated into this RFC, where
relevant.

## Requirements

These are the operations dictionaries MUST support, which we have derived by
analyzing the use cases and generalizing from existing grouping APIs:

-   *First-class*: Dictionaries are a first-class concept in the CF APIs, and
    shall be represented as a capability.
-   *Aggregation*: There is an "aggregate" operation to construct a dictionary
    from a set of capabilities.
-   *Extraction*: There is an "extraction" operation to extract an individual
    capability from a dictionary, which can be routed and consumed like any
    capability. This is roughly the inverse of aggregation.
-   *Delegation*: Dictionaries can be passed between components.
-   *Nesting*: Since dictionaries are a capability, dictionaries can contain
    other dictionaries.
-   *Structure*: Dictionaries are tagged with metadata that indicates precisely
    what capabilities they contain.
-   *Extension*: There is an operation to construct a new dictionary `B'` that
    inherits the contents of `B`, and adds additional capabilities.
-   *Mutability*: The contents of a dictionary may change over time. However,
    higher-level policy may exist that places constraints on the mutability of
    particular dictionaries.

## Design

### Definition

A *dictionary* is defined as a bag of key/value pairs, where the key is a
*capability name* (e.g., `fuchsia.example.Echo`) and the value is a CF
capability.

A [capability name][docs-capability-name] is a sequence of one or more
characters from the set `[A-Za-z0-9_-.]`, of size 1 to N (currently N = 100, but
we may extend it in the future).

### Dictionaries at runtime

We will introduce a public FIDL protocol that provides an interface to a
dictionary. As FIDL-pseudocode:

```
library fuchsia.component;

type DictionaryEntry = resource struct {
    key DictionaryKey;
    value Capability;
};

protocol Dictionary {
    Insert(DictionaryEntry) -> ();
    Remove(DictionaryKey) -> (Capability);
    Lookup(DictionaryKey) -> (Capability);
    Enumerate() -> Iterator<DictionaryKey>;
    Clone();
};
```

We will also introduce a public discoverable FIDL protocol that allows the
caller to create an empty dictionary.

Follow-up design work will determine the precise type definition for
`Capability`.

### Dictionaries in component declarations {#formalization}

Let's begin with a bit of formalization that will help with defining the
operations. We'll define four special dictionaries associated with every
component: a *component input dictionary*, *component output dictionary*,
*program input dictionary*, and *program output dictionary*. Together, these are
the *root dictionaries*.

The *component input dictionary* is a dictionary that contains all capabilities
`offer`ed to the component by its parent; or in other words, all capabilities
that a component can route `from parent`.

The *component output dictionary* is a dictionary that contains all capabilities
`expose`d by a component to its parent; or in other words, all capabilities that
the parent can route `from #component`.

The *program input dictionary* is a dictionary containing all capabilities
`use`d by a component.

The *program output dictionary* is a dictionary containing all of a component's
`capability` declarations.

We can express the capability routing operations in terms of these definitions:

-   `use`, `offer`, `expose`, and `capabilities` are *routing operations* that
    route *capabilities* between *dictionaries*.
    -   `use` routes a capability from a *root dictionary* to the *program input
        dictionary*.
    -   `expose` routes a capability from a *root dictionary* to the *component
        output dictionary*.
    -   `offer` routes a capability from a *root dictionary* to a child's
        *component input dictionary*.
    -   `capabilities` makes a capability in the *program output dictionary*
        available for routing.

With this design, we will generalize this to allow routing operations to use
arbitrary dictionaries as a source, not just root ones:

-   `use` routes a capability from a *dictionary* to the *program input
    dictionary*.
-   `expose` routes a capability from a *dictionary* to the *component output
    dictionary*.
-   `offer` routes a capability from a *dictionary* to a child's *component
    input dictionary* **or another dictionary**.

#### Declaring

To define a brand new, empty dictionary:

```json5
capabilities: [
    {
        dictionary: "diagnostics-bundle",
    },
],
```

To define a dictionary whose contents are inherited from an existing dictionary,
provide a path to that dictionary with `extends` (see
[Extension](#main-extension)):

```json5
    {
        dictionary: "diagnostics-bundle",
        extends: "parent/logging-bundle/sys",
    },
```

#### Aggregation {#main-aggregation}

To aggregate capabilities into a dictionary, route capabilities into it with
the `to` keyword in an `offer`. Since the dictionary is defined by this
component, the root dictionary containing it is `self`. All the same keywords
are supported as a regular `offer`. For example, you can change the
capability's name in the target dictionary using the `as` keyword.

```json5
capabilities: [
    {
        dictionary: "diagnostics-bundle",
    },
],
offer: [
    {
        protocol: "fuchsia.logger.LogSink",
        from: "#archivist",
        to: "self/diagnostics-bundle",
    },
    {
        protocol: "fuchsia.inspect.InspectSink",
        from: "#archivist",
        to: "self/diagnostics-bundle",
    },
    {
        directory: "publisher",
        from: "#debugdata",
        to: "self/diagnostics-bundle",
        rights: [ "r*" ],
        as: "coverage",
    },
],
```

#### Delegation

Delegation means routing a dictionary:

```json5
offer: [
    {
        dictionary: "diagnostics-bundle",
        from: "parent",
        to: "#session",
    },
],
```

Like with other capability routes, you can change the name for the target using
the `as` keyword.

```json5
offer: [
    {
        dictionary: "logging-bundle",
        from: "parent",
        to: "#session",
        as: "diagnostics-bundle",
    },
],
```

An equivalent runtime API will be available for
[dynamic offers][rfc-dynamic-offers].

#### Nesting

Dictionaries can be made to contain other dictionaries, by routing them into
another dictionary using the [aggregation](#main-aggregation) syntax:

```json5
capabilities: [
    {
        dictionary: "session-bundle",
    },
],
offer: [
    {
        dictionary: "driver-services-bundle",
        from: "parent",
        to: "self/session-bundle",
    },
],
```

#### Extraction {#main-extraction}

Following the [formalization](#formalization), we will we will extend the `from`
keyword to accept not only a root dictionary, but a dictionary that's nested in
a root dictionary.

To extract a capability from a dictionary, name the dictionary in `from`. This
dictionary is relative to a root dictionary (`parent`, `#child`, etc.)

```json5
offer: [
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "parent/session-bundle",
        to: "#window_manager",
    },
],
```

This also works for `use`:

```json5
use: [
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "parent/session-bundle",
    },
],
```

Extraction also works when dictionaries are nested in other dictionaries:

```json5
use: [
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "parent/session-bundle/gfx",
    },
],
```

#### Extension {#main-extension}

Use the `extends` option in a dictionary definition to inherit from another
dictionary:

```json5
capabilities: [
    {
        dictionary: "session-bundle",
        // `session-bundle` is initialized with the dictionary the parent
        // offered to this component, also called `session-bundle`.
        extends: "parent/session-bundle",
    },
],
offer: [
    {
        dictionary: "session-bundle",
        from: "self",
        to: "#session-manager",
    },
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "#ui",
        to: "self/session-bundle",
    },
],
```

#### Mutability

Dictionaries constructed declaratively are immutable, which is a useful
[security property](#security-considerations). For a dictionary to be mutable it
must be created at [runtime](#dictionaries-at-runtime).

### Metadata of capabilities in dictionaries

When capabilities are put into a dictionary, they retain all their type
information and metadata, which is separate from any metadata associated with
the dictionary itself.

For example, if a capability with [`optional` availability][docs-availability]
is added to a dictionary by component `A`, and component `B` extracts that
capability, it will have `optional` availability at the point of extraction,
even if the availability of the dictionary itself is `required`.

We may impose certain constraints on availability at the point of aggregation.
For example, it could make sense to forbid putting a `required` capability into
an `optional` dictionary, since this would violate the usual invariant that
availability never gets weaker when routing from target to source.

### Interoperability between runtime and declarative dictionaries

Dictionaries created at runtime must be interoperable with dictionaries in
component declarations. If this were not the case, it would force users into
exclusively choosing one or the other, and would be evidence that the conceptual
foundation of bundling was not sufficiently general to solve both types of use
cases in a similar way.

The details of the design for interoperability will be the subject of a followup
proposal.

The primary known use case for this feature is driver framework, for routing
service bundles that are populated at runtime.

## Implementation

Landing dictionaries in cml will follow the usual pipeline for introducing new
cml features. First, we will add dictionaries to the cml and component.decl
schema. Then, we will update `cmc`, `cm_fidl_validator`, and `cm_rust`, and
`realm_builder` to compile, validate, and represent dictionaries. Scrutiny will
also be updated to recognize dictionaries and have the ability to validate
dictionary routes.

Work is already in progress to integrate dictionaries (as a rust type) into the
component model and routing engine. The implementation of the dictionaries API
should build upon this work to use these dictionaries as the backend for the
public dictionary API and as the transport for routing dictionary capabilities.

## Performance

There are no special performance considerations. Routing a dictionary should be
as fast or faster than it takes to route the constituent capabilities
individually.

## Ergonomics

Improving the ergonomics of building component topologies was a major motive for
this design.

While introducing a new feature naturally increases the complexity of the API,
we believe this will be more than offset by the reduction in complexity gained
by incorporating dictionaries in topologies.

## Backwards Compatibility

There is no versioning support in cmc yet for component manifest features, so
care must be taken to avoid breaking compatibility with pre-built manifests.
Fortunately, all new syntax being introduced for dictionaries is compatible with
the old syntax, so that makes the job easier. For example, the current name
syntax in `from` becomes a special case of the new path syntax.

If part of a capability route passes through a dictionary, any security policies
pertaining to that capability must still apply.

## Security considerations

When capabilities are routed in a dictionary, some transparency is lost because
the identities of the capabilities inside the dictionary are hidden from the
intermediate components in the routes. However, they can still be deduced by
following the route backwards to the provider(s). This is a deliberate
compromise to achieve the flexibility and power that dictionaries unlock.

Declaratively-constructed dictionaries are immutable. For these dictionaries,
you can obtain a complete description of their contents by performing a
depth-first search of the aggregate routes of the dictionary from the target to
its sources.

If and when dictionaries replace environments, it will enhance the security
posture of the system because dictionaries, unlike environments, are routed
explicitly and in the same way as other capabilities.

## Privacy considerations

This proposal has no impact on privacy.

## Testing

We will test this like most component manager features, with unit tests in
`component_manager` and `cmc`, and integration tests in
`component_manager/tests`. We will also add integration tests to scrutiny that
exercise dictionaries and policies applied to routes with dictionaries.

## Documentation

We'll update the rustdoc in `//tools/lib/cml`.

Add a page to `//docs/concepts/components` to explain dictionaries.

Add an example to `//examples/components`.

## Drawbacks, alternatives, and unknowns

### Alternative 1: `in` and `into` keywords for dictionaries

#### Declaring

A *dictionary capability* is a cml/component.decl capability type that grants
access to a dictionary.

You declare a dictionary like any other component framework capability. are two
variants of dictionary creation, determined by the presence of an `extends`
keyword.

First, you can define a brand new, empty dictionary:

```json5
capabilities: [
    {
        dictionary: "diagnostics-bundle",
    },
],
```

Or, you can define a dictionary whose contents are inherited from an existing
dictionary by specifying a path to a dictionary in `extends` and a source in
`from` (see [Extension](#alt-extension)):

```json5
    {
        dictionary: "diagnostics-bundle",
        extends: "logging-bundle/sys",
        from: "parent",
    },
```

#### Aggregation

To aggregate capabilities into a dictionary, route capabilities into it with the
`into` keyword:

```json5
capabilities: [
    {
        dictionary: "diagnostics-bundle",
    },
],
offer: [
    {
        protocol: "fuchsia.logger.LogSink",
        from: "#archivist",
        into: "diagnostics-bundle",
    },
    {
        protocol: "fuchsia.inspect.InspectSink",
        from: "#archivist",
        into: "diagnostics-bundle",
    },
    {
        protocol: "fuchsia.debugdata.Publisher",
        from: "#debugdata",
        into: "diagnostics-bundle",
    },
],
```

#### Delegation

Same as main design.

#### Nesting

Dictionaries can be made to contain other dictionaries, simply by routing them
into a dictionaries using the aggregation syntax:

```json5
capabilities: [
    {
        dictionary: "session-bundle",
    },
],
offer: [
    {
        dictionary: "driver-services-bundle",
        from: "parent",
        into: "session-bundle",
    },
],
```

#### Extraction {#alt-extraction}

We introduce a new `in` keyword that designates a dictionary to extract a
capability from.

When `in` is present, the capability keyword (`protocol` etc.) refers to a
capability in this dictionary, rather than a capability directly offered by the
component named in `from`.

`in` may be a name or a path (where name can be viewed as a degenerate case). If
it is a name, it refers to a dictionary presented by `from`. If it's a path, the
first segment of the path designates a dictionary presented by `from`, while the
rest of the path designates a path to a dictionary nested in this one.

`in` is supported by `use`, `offer`, `expose`. It is always optional.

```json5
offer: [
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "parent",
        in: "session-bundle/gfx",
        to: "#window_manager",
    },
],
```

```json5
expose: [
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "#scenic",
        in: "gfx-bundle",
    },
],
```

```json5
use: [
    {
        protocol: "fuchsia.ui.composition.Flatland",
        in: "session-bundle/gfx",
    },
],
```

#### Extension {#alt-extension}

Use the `extends` keyword in a dictionary definition to inherit from another
dictionary:

```json5
capabilities: [
    {
        dictionary: "diagnostics-bundle",
        extends: "parent/logging-bundle/sys",
    },
],
offer: [
    {
        dictionary: "diagnostics-bundle",
        from: "self",
        to: "#session-manager",
    },
    {
        protocol: "fuchsia.tracing.provider.Registry",
        from: "#trace_manager",
        into: "diagnostics-bundle",
    },
],
```

### Alternative 2: Paths in capability identifiers

Instead of designating the path to the dictionary in `from`, the path could be
part of the capability identifier (`protocol`, `directory`, etc.)

#### Aggregation

Same as main design.

#### Delegation

Same as main design.

#### Nesting

Same as main design.

#### Extraction

To extract a capability from a dictionary, specify the path to the capability
within the dictionary in the capability identifier (`protocol`, `directory`,
etc.).

```json5
offer: [
    {
        protocol: "session-bundle/fuchsia.ui.composition.Flatland",
        from: "parent",
        to: "#window_manager",
    },
],
```

The name in the target will be the last path element, or "dirname", by default
(`fuchsia.ui.composition.Flatland`). Or you can rename it with `as`:

```json5
offer: [
    {
        protocol: "session-bundle/fuchsia.ui.composition.Flatland",
        from: "parent",
        to: "#window_manager",
        as: "fuchsia.ui.composition.Flatland-windows",
    },
],
```

This also works for `use`, which makes a capability from a dictionary available
to the program. Like with other `use` declarations, the default target path
rebases the name (last path element) upon `/svc`:

```json5
use: [
    {
        protocol: "session-bundle/fuchsia.ui.composition.Flatland",
        path: "/svc/fuchsia.ui.composition.Flatland",  // default
    },
],
```

The path syntax also works with dictionaries nested in other dictionaries:

```json5
use: [
    { protocol: "session-bundle/gfx/fuchsia.ui.composition.Flatland" },
],
```

#### Extension

Use the `origin: #...` option in a dictionary definition to inherit from another
dictionary:

```json5
capabilities: [
    {
        dictionary: "session-bundle",
        // Source of the dictionary to extend (in this case, the one named
        // "session-bundle" from the parent)
        origin: "#session",
        from: "parent",
    },
],
offer: [
    {
        dictionary: "session-bundle",
        from: "self",
        to: "#session-manager",
    },
    {
        protocol: "fuchsia.ui.composition.Flatland",
        from: "#ui",
        into: "session-bundle",
    },
],
```

### Alternative 3: Capability identifiers become paths

#### Names -> Paths

Officially, capability identifiers in cml are names, with no intrinsically
nested structure. For example:

```json5
offer: [
    {
        protocol: "fuchsia.fonts.Provider",
        from: "#font_provider",
        to: "#session-manager",
    },
],
```

Capability identifiers, however, do get mapped to paths, in the `capabilities`
and `use` section. For protocols, this is usually implicit: if no path is
provided, cmc fills in a default path of `/svc/${capability-name}`. For example:

```json5
use: [
    {
        protocol: "fuchsia.fonts.Provider",
        // path in namespace
        path: "/svc/fuchsia.fonts.Provider",
    },
],
```

```json5
capabilities: [
    {
        protocol: "fuchsia.fonts.Provider",
        // path in outgoing directory
        path: "/svc/fuchsia.fonts.Provider",
    },
],
```

This alternative would *support paths in capability identifiers*. More formally:

-   A *capability identifier* is a sequence of one or more *names* from the
    character set `[A-Za-z0-9_-.]`, containing 1 to 100 characters and separated
    by `/` characters. Leading `/` is not allowed.
    -   Or, in regex syntax: `[A-Za-z0-9_-]{1,100}(/[A-Za-z0-9_-]{1,100})*`
    -   Existing capability identifiers are forward-compatible with the new
        syntax.

Below, we will see how this syntax naturally lays the groundwork for bundling.

#### Aggregation

To aggregate capabilities into a dictionary, route them with the same path
prefix:

```json5
offer: [
    {
        protocol: "fuchsia.logger.LogSink",
        from: "#archivist",
        to: "all",
        as: "diagnostics/fuchsia.logger.LogSink",
    },
    {
        protocol: "fuchsia.inspect.InspectSink",
        from: "#archivist",
        to: "all",
        as: "diagnostics/fuchsia.inspect.InspectSink",
    },
    {
        protocol: "fuchsia.debugdata.Publisher",
        from: "#debugdata",
        to: "all",
        as: "diagnostics/fuchsia.debugdata.Publisher",
    },
],
```

#### Delegation

Delegation is simply routing a dictionary as is:

```json5
offer: [
    {
        dictionary: "diagnostics",
        from: "parent",
        to: "#session",
    },
],
```

#### Nesting

Dictionaries can be made to contain other dictionaries, by making the nesting
dictionary's path a prefix of the nested dictionary:

```json5
offer: [
    {
        dictionary: "driver-services-bundle",
        from: "parent",
        to: "#session-manager",
        as: "session/driver-services",
    },
],
```

#### Extraction

Simply name the capability within the dictionary you want to extract it from:

```json5
offer: [
    {
        protocol: "session-bundle/fuchsia.ui.composition.Flatland",
        from: "parent",
        to: "#window_manager",
        as: "fuchsia.ui.composition.Flatland",
    },
],
```

This also works for `use`:

```json5
use: [
    {
        protocol: "session-bundle/fuchsia.ui.composition.Flatland",
        path: "/svc/fuchsia.ui.composition.Flatland",
    },
],
```

Extraction also works when dictionaries are nested in other dictionaries (TODO:
example)

#### Extension

Rename capabilities to have a path prefix coincident with a dictionary:

```json5
offer: [
    {
        protocol: "session-bundle",
        from: "parent",
        to: "#session-manager",
    },
    {
        protocol: "fuchsia.ui.composition.Flatland",
        as: "session-bundle/fuchsia.ui.composition.Flatland",
        from: "#ui",
        to: "#session-manager",
    },
],
```

### Why dictionaries instead of directories?

Instead of introducing dictionaries, we could use `fuchsia.io` directories as
the base type for bundles. In one respect, this is attractive: directories
already exist, and provide their own form of hierarchical bundling. However,
there are many arguments against using directories:

-   The VFS type system carries different information than the CF type system;
    for example, services, directories, and storage would all map to
    subdirectories in VFS, even though they are different types in CF.
-   The interface for directories is considerably more complex than what is
    needed to support capability bundling. Features like NODE_REFERENCE, links,
    flags, attributes, data files, etc. are not relevant for bundling use cases.
-   The size of the VFS library is too large for some applications, particularly
    drivers. It is for this reason that the
    [`//sdk/lib/component/outgoing`][src-outgoing] library links in a shared
    library shim ([`//sdk/lib/svc`][src-svc]) instead of the VFS library
    directly, at the cost of functionality and transparency.
-   There is not one VFS implementation, but two separate C++ implementations
    and one Rust implementation. These implementations have subtle differences
    and feature gaps. This is not a problem with dictionaries because there is a
    single implementation of dictionaries, in the component runtime.
-   Directories would make it more challenging to write codegen bindings that
    represent each capability to provided or consumed by a program as a discrete
    language element.
-   Directories don't naturally support the "aggregation" or "extension"
    operations. They must be simulated by serving a new directory where some
    nodes redirect to the old ones, which is non-trivial to implement and prone
    to error.

## Future work

A companion design will be proposed separately to target the driver use cases,
which can't be completely solved with just the features in this proposal.

This design opens the door to a more economical syntax for capability routing.
Instead of having the capability name and `from` be separate properties, we
could combine them into a single path, whose root is conceptually a dictionary
that contains all the root dictionaries. For example:

```
offer: [
    {
        protocol: "#ui/fuchsia.ui.composition.Flatland",
        to: "#session-manager",
    },
],
```

This syntax has a nice property: it naturally generalizes to allowing one to
route an entire root dictionary:

```
offer: [
    {
        // Plumb all capabilities from parent to child #session-manager
        dictionary: "parent",
        to: "#session-manager",
    },
],
```

It's also worth mentioning a more general version that would unify the syntax
even more:

```
route: [
    {
        // Path to source capability in dictionary
        src: "#ui/fuchsia.ui.composition.Flatland",
        // Path of target capability in dictionary
        dst: "#session-manager/fuchsia.ui.composition.Flatland",
    },
],

route: [
    {
        src: "parent",
        dst: "#session-manager/parent",
    },
],
```

## Prior art and references

Capability bundles are an old idea. There are many internal predecessor docs
that propose similar ideas.

[docs-availability]: https://fuchsia.dev/reference/cml#offer
[docs-capability-name]: https://fuchsia.dev/reference/cml#names
[docs-component-environments]: /docs/concepts/components/v2/environments.md
[docs-component-manifests]: /docs/concepts/components/v2/component_manifests.md
[docs-component-resolvers]: /docs/concepts/components/v2/capabilities/resolver.md
[docs-component-runners]: /docs/concepts/components/v2/capabilities/runner.md
[docs-component-services]: /docs/concepts/components/v2/capabilities/service.md
[docs-process-namespace]: /docs/concepts/process/namespaces.md
[fidl-component-exposed-dir]: https://fuchsia.dev/reference/fidl/fuchsia.component#Realm.OpenExposedDir
[rfc-dynamic-offers]: /docs/contribute/governance/rfcs/0107_dynamic_offers.md
[src-outgoing]: /sdk/lib/component/outgoing/cpp
[src-svc]: /sdk/lib/svc
