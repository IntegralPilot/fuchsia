# Availability

To select what capabilities go in its [namespace][glossary.namespace], a
component specifies a [`use`][docs.use] declaration for each of those
capabilities in its [manifest][docs.component-manifests]. However, since
[capability routing][docs.capability-routing] involves a chain of components,
just because a component `use`s a capability doesn't mean it will always be
available. It can be unavailable for various reasons: for example, if one of the
components in the chain didn't route the capability, or if one of those
components failed to resolve.

When a component `use`s or routes a capability, it may have various expectations
around whether the capability must be available. In some cases, a capability is
essential to a component's operation: if someone creates a topology is that
fails to route the capability to a component, something should ideally detect
that failure as early as possible to prevent a bug. However, in other cases, the
component may be tolerant of the capability's absence, or the capability may be
expected to be absent in certain configurations. In order to accommodate these
different scenarios, [cml][docs.cml] provides an `availability` option on
[`use`][docs.use], [`offer`][docs.offer], and [`expose`][docs.expose] which the
component can use to declare its expectation about a capability's availability.

## Options

The framework supports the following options for `availability`:

-   [`required`](#required)
-   [`optional`](#optional)
-   [`transitional`](#transitional)
-   [`same_as_target`](#same-as-target)

### Required

The most commonly used `availability` option is `required`, which indicates that
the component always expects the capability to be available, and cannot be
expected to function properly if it's absent. This is the default option that is
automatically set if no availability is specified.

### Optional

`optional` signifies that a capability may not be present on some
configurations. If a component `use`s an optional capability that turns out to
be unavailable, the component should be able to function without it, for example
by deactivating any features requiring that capability.

However, if an `optional` capability is not available on some configuration, the
framework expects that the topology still includes a complete route for the
capability that terminates in [`void`][docs.offer]. In this way, the product
owner is expected to make an explicit choice that either the capability is
available (its route terminates at some provider component) or it is not (its
route terminates at `void`). If the route is incomplete,
[tooling and diagnostics](#tooling-and-diagonstics) will report this as an
error.

### Transitional

`transitional` is like a weaker version of `optional`. Like `optional`, if a
component uses a `transitional` capability, it should be tolerant to the absence
of the capability. However, unlike `optional`, a `transitional` route does not
need to terminate in `void` and
[tooling and diagnostics](#tooling-and-diagonstics) will not report any error if
a `transitional` route is incomplete or invalid.

The name "transitional" connotes the idea that this option is useful for soft
transitions. For example, suppose you have an `Echo` protocol, that you want to
replace with `EchoV2`. At an early step in this transition, you can change the
client to `use` the new protocol with `transitional` availability. Later, once
an end-to-end route has been established, you can upgrade the availability to
`required` or `optional`. If you had tried to use `optional` instead,
[tooling and diagnostics](#tooling-and-diagnostics) would complain that the
client component's parent was missing a capability route for `Echo2`, while
`transitional` will suppress such warnings.

### Same as target

`same_as_target` acts as a passthrough: it causes the availability to be
inherited from whatever availability was set by the route's target component.
This is useful for intermediate components that pass a lot of capabilities
through, so that when the `availability` of a route is changed in the target, no
change is required in the source.

Because the component that `use`s a capability is its final destination,
`same_as_target` is not a valid option for `use`.

## Availability comparisons

There is a relative ordering of "strength" between the availability options.
From strongest to weakest, the order is: `required > optional > transient`.
`same_as_target` is not part of this because it's a passthrough: the strength of
a `same_as_target` capability route is effectively equal to whatever
availability it inherited from the target.

It is valid, and useful in many cases, to *downgrade* the availability from the
source to the target. For example, if component `X` exposes a `required`
capability to `Y`, `Y` may choose to `use` the capability as `optional`, or
`offer` the capability as `optional` to another child. However, it is a routing
error to *upgrade* the availability from the source. If this happens, any
attempt to use or route the capability will fail, in the same way it would fail
if the routing chain were incomplete. This is an error because it means you are
trying to establish stronger guarantees on the availability than what the source
has guaranteed. For example, it doesn't make sense for a component to `use` a
capability as `required` if its parent `offer`ed the capability as `optional`,
since that means the parent has declared the capability might not be available.

## Tooling and diagnostics

The main effect of setting availability is to control how host tooling and
diagnostics reports routing errors. In a nutshell, this means the weaker the
`availability`, the less strict they will be. Here's the detailed specification:

-   If a `required` capability route is incomplete, invalid, or ends in `void`:
    -   [Scrutiny][src.scrutiny] will report this as a routing error. This will
        cause a build error if the build configuration required this to pass.
    -   `component_manager` will log a `WARNING` message in the using
        component's scope describing the routing error.
-   If an `optional` capability route is incomplete or invalid:
    -   [Scrutiny][src.scrutiny] will report this as a routing error. This will
        cause a build error if the build configuration required this to pass.
    -   `component_manager` will log an `INFO` message in the using component's
        scope describing the routing error.
-   If an `optional` capability route is ends in `void`:
    -   [Scrutiny][src.scrutiny] will not report an error.
    -   `component_manager` may log an `INFO` message, but no error.
-   If a `transitional` capability route is incomplete, invalid, or ends in
    `void`:
    -   No error or other information is logged.

For general runtime behavior, `availability` has no effect. For example, suppose
a component attempts to `use` a protocol in its namespace whose route is broken.
The end result will be the same if the protocol was routed as `required` or
`optional`:

-   The protocol will appear in the component's namespace.
-   The initial call to connect to the capability will succeed (assuming a
    standard one-way API is used like
    [connect_to_protocol][src.connect_to_protocol] in rust).
-   `component_manager` will attempt to route the capability. In the end,
    routing will fail, and the channel will be closed with a `NOT_FOUND`
    [epitaph][docs.epitaph].

## source_availability: unknown

[cml][docs.cml] has an additional feature which can be used to autogenerate
either a `required` or `optional void` `offer`, depending on whether a
[cml shard][docs.includes] is included that has a child declaration for the
source. More information on this feature is in the
[build docs][docs.inter-shard-deps].

## target_availability: unknown

[cml][docs.cml] has an additional feature which can be used to omit an `offer`,
depending on whether a [cml shard][docs.includes] is included that has a child
declaration for the target. More information on this feature is in the [build
docs][docs.inter-shard-deps].

## Example

The following example illustrates the concepts described in this doc.

In this example, there are three components:

-   `echo_client`, which tries to consume various protocols provided by
    `echo_server`
-   `echo_server`, which provides some protocols
-   `echo_realm`, the parent of `echo_client` and `echo_server` which links them
    together

Let's examine their cml files:

```json5
// echo_client.cml
{
    ...
    use: [
        { protocol: "fuchsia.example.Echo" },
        {
            protocol: "fuchsia.example.EchoV2",
            availability: "transitional",
        },
        {
            protocol: "fuchsia.example.Stats",
            availability: "optional",
        },
    ],
}

// echo_server.cml
{
    ...
    capabilities: [
        { protocol: "fuchsia.example.Echo" },
    ],
    expose: [
        {
            protocol: "fuchsia.example.Echo",
            from: "self",
        },
    ],
}

// echo_realm.cml
{
    offer: [
        {
            protocol: "fuchsia.example.Echo",
            from: "#echo_server",
            to: "#echo_client",
            availability: "required",
        },
        {
            protocol: "fuchsia.example.Stats",
            from: "void",
            to: "#echo_client",
            availability: "optional",
        },
    ],
    children: [
        {
            name: "echo_server",
            url: "echo_server#meta/echo_server.cm",
        },
        {
            name: "echo_client",
            url: "echo_client#meta/echo_client.cm",
        },
    ],
}
```

Recall that when `availability` is omitted, it defaults to `required`. With this
topology, the behavior is as follows:

-   `echo_client` will be able to successfully connect to
    `fuchsia.example.Echo`.
-   `echo_client` would fail to connect to `fuchsia.example.EchoV2` or
    `fuchsia.example.Stats`.
-   Tooling and diagnostics will not log an error.
    -   No error for `fuchsia.example.Stats` because it's `optional` and routes
        to `void`.
    -   No error for `fuchsia.example.Stats` because it's `transient`.

Now, let's see what happens with a different version:

```json5
// echo_client.cml
{
    ...
    use: [
        { protocol: "fuchsia.example.Echo" },
        {
            protocol: "fuchsia.example.EchoV2",
            availability: "transitional",
        },
        {
            protocol: "fuchsia.example.Stats",
            availability: "optional",
        },
    ],
}

// echo_server.cml
{
    ...
    capabilities: [
        {

            protocol: [
                "fuchsia.example.Echo",
                "fuchsia.example.EchoV2",
                "fuchsia.example.Stats",
            ],
        },
    ],
    expose: [
        {
            protocol: [
                "fuchsia.example.Echo",
                "fuchsia.example.EchoV2",
            ],
            from: "self",
        },
        {
            protocol: "fuchsia.example.Stats",
            from: "self",
            availability: "optional",
        },
    ],
}

// echo_realm.cml
{
    offer: [
        {
            protocol: [
                "fuchsia.example.Echo",
                "fuchsia.example.EchoV2",
            ],
            from: "#echo_server",
            to: "#echo_client",
            availability: "same_as_target",
        },
        {
            protocol: "fuchsia.example.Stats",
            from: "#echo_server",
            to: "#echo_client",
            availability: "optional",
        },
    ],
    children: [
        {
            name: "echo_server",
            url: "echo_server#meta/echo_server.cm",
        },
        {
            name: "echo_client",
            url: "echo_client#meta/echo_client.cm",
        },
    ],
}
```

Now:

-   `echo_client` will be able to successfully connect to
    `fuchsia.example.Echo`, `fuchsia.example.EchoV2`, and
    `fuchsia.example.Stats`.
    -   Each route, while having different `availability`, is complete and
        terminates in a real component.
    -   For each route, availability between source and target passes the
        [comparison check](#availability-comparisons)
-   Tooling and diagnostics will not log an error, because all routes complete.

[docs.cml]: https://fuchsia.dev/reference/cml
[docs.use]: https://fuchsia.dev/reference/cml#use
[docs.offer]: https://fuchsia.dev/reference/cml#offer
[docs.epitaph]: /docs/reference/fidl/language/language.md#protocols
[docs.includes]: https://fuchsia.dev/reference/cml#include
[docs.expose]: https://fuchsia.dev/reference/cml#expose
[docs.inter-shard-deps]: /docs/development/components/build.md#inter-shard-dependencies
[docs.capability-routing]: /docs/concepts/components/v2/capabilities/README.md#routing
[docs.component-manifests]: /docs/concepts/components/v2/component_manifests.md
[glossary.namespace]: /docs/glossary/README.md#namespace
[src.connect_to_protocol]: /src/lib/fuchsia-component/client/src/lib.rs
[src.scrutiny]: /src/developer/ffx/plugins/scrutiny/verify/README.md
