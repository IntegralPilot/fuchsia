<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0277" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Service capabilities and dictionary capabilities both solve similar issues: they
provide a way to route and consume bundles of capabilities, where each
capability in the bundle may or may not come from the same source.

Having two separate means to bundle capabilities, with subtly different
semantics and notable differences in their respective feature sets, results in a
more complex system that's both harder to understand and harder to maintain.

Furthermore, the component framework concept of services is tech debt. [Services
were introduced][services-rfc] to solve an important and singular problem: that
of supporting multiple components providing the same protocol(s) in the driver
framework. Services as we know them today are thus a poor fit for many other
areas in Fuchsia. A lack of dynamism in the tooling available to developers
meant that services were the only tool available to accomplish developer goals,
and they were used in ways that work around their design instead of with it.

This tech debt is likely to grow over time, as services still remain the only
way to do some key things such as dynamic capability aggregation.

## Summary

By adding some key features around dynamic aggregation to dictionary
capabilities, we can make service capabilities redundant. This will allow us to
fully migrate off of service capabilities and deprecate the feature in the
component framework, leaving one approved and featureful way to route around
bundles of capabilities.

## Stakeholders

_Facilitator:_ (currently unassigned)

_Reviewers:_

- Adam Barth (Starnix)
- Eric Stone (Security)
- Ian McKellar (FIDL)
- Suraj Malhotra (Driver Framework)

_Socialization:_

This design went through multiple feedback rounds with reviewers on Google Docs
before being transformed into an RFC document.

## Requirements

### FIDL services can still be published and used at the same paths

Any component which is publishing a FIDL service in its outgoing directory, or
consuming a FIDL service in its namespace, can continue to publish and consume
the same VFS items at the exact same paths, with the exact same runtime
semantics.

### All service use cases remain fully supported

Any way in which FIDL services are currently used can still be accomplished
easily. This includes:

- Aggregated services where the instances are anonymized.
- Aggregated services where the instances are renamed/filtered.
- Non-aggregated services, where two components are communicating directly
  without going through a component manager hosted VFS.
- Multi-protocol service definitions, where service instances hold multiple
  protocols.
- Single-protocol service definitions, where service instances hold a single
  protocol.

### FIDL services as a concept are unchanged, and still fully supported

Any component which is currently relying on services as a concept at the FIDL
layer can continue to do so, and all of the FIDL tooling relating to services
continues to exist and be supported.

### Dictionary contents are predictable

Dictionaries are a general tool, and can hold any number of any type of
capability. This flexibility must not make it overly difficult to reason about
the system, especially when it comes to the possible contents of a dictionary.
In short: a dictionary's contents must be predictable and auditable.

## Background

### FIDL service capabilities

A service capability at the FIDL level is a grouping of protocol capabilities
(referred to as "service members"). When used at runtime, a single component may
publish zero or more of these groupings simultaneously. These groupings are
referred to as "service instances", and each take the form of a directory node
in the top-level service directory. Inside each service instance are the set of
service nodes listed in the FIDL definition of the service (the service
members).

This means to open a specific protocol inside of a service, a developer needs to
know the name of the FIDL service, the name of the service instance, and the
name of the service member (i.e. protocol) they wish to open.

```
/svc/$Service/$Instance/$Member
```

This specific directory layout, with the service capability holding instances
which themselves hold members, is referred throughout this document as the "FIDL
services contract", as both provider and consumer of a service agree by
convention that this is how a service capability directory is laid out.

### Component framework service capabilities

Component framework supports component manifests declaring, routing, and using
service capabilities.

When one component declares it has a service capability, which is routed to
another component that uses it, component manager performs no enforcement of the
FIDL directory layout. From component manager's perspective it's simply a
read-only directory, and it's up to the two components to agree on the internal
structure of that directory.

The key feature of service capabilities at the component framework level is that
in addition to the typical point-to-point capability routing (as described in
the last paragraph), service capabilities may be aggregated together into a
single service capability that itself holds service instances from multiple
backing service capabilities that make up the aggregate.

There are two different strategies by which service capabilities may be
aggregated together: anonymizing and renaming. Anonymizing aggregates hold every
single service instance from the backing service capabilities, and each service
instance gets a random (i.e. anonymous) name. Renaming aggregates have renaming
rules for the service instances to describe the instance names in the aggregated
service and which service instances in the backing service capabilities they
correlate to. Any service instances in the backing service capabilities not
mentioned in a rename rule do not appear in the aggregate, so renaming
aggregates can also be used to filter the set of instances in a service.
Anonymous aggregates are always comprised of at least two backing service
capabilities, while renaming aggregates are always comprised of at least one.

Unlike with point-to-point service capability routing, when a service capability
is part of an aggregate component manager does expect the component providing
the service to correctly publish service instances inside of the top-level
directory, as component manager will potentially search the directory for them
and will forward open requests to them that the aggregate receives.

### Integration (and lack thereof) with FIDL service feature

Services at the FIDL layer and services at the component framework layer are far
less dependent on each other than one might initially expect.

The FIDL layer provides a dedicated construct in the FIDL language to define the
members that will be in a given service, and services defined in FIDL will then
have tooling generated for them in the same way that code generation works for
FIDL protocols. This tooling, combined with support from other libraries such as
`fuchsia_component_server`, enables components to publish FIDL services in their
outgoing directories and consume FIDL services in their namespaces.

At the component framework layer, two components relying on a service to
communicate between them could declare, route, and consume a directory
capability instead of a service capability and there would be no perceptible
runtime change. Likewise, two components that wish to communicate over a
read-only directory that doesn't conform to FIDL service expectations could
declare, route, and consume a service capability instead of a directory and
again this would not result in detectable changes from the program's
perspective. This is due to the lack of enforcement performed by component
manager on service directory layout mentioned in the previous section.

One can thus use FIDL services without CF services, and vice versa, even though
this is rather unheard of on Fuchsia today.

### Dictionary capabilities

Dictionary capabilities [were introduced][dictionaries-rfc] as a generic way to
bundle together multiple named capabilities and route them around under a single
name. Through capability routing capabilities of any type may be added to a
dictionary, or removed from a dictionary and then individually routed.

Where service capabilities are (when aggregated) required to hold directories
which by convention hold protocols, dictionaries have no such limitations.
Dictionaries can hold directories, protocols, event streams, runners, even other
dictionaries.

Dictionaries may even be dynamically created by components, where a component
uses CF APIs to programmatically populate a dictionary that is then routed, and
use dictionaries, where a dictionary's contents are made available through the
component's namespace. Note that both of these features are currently limited by
an allowlist.

## Design

### Anonymized aggregation of dictionaries

To mirror the "anonymized aggregate" feature of services, we will add the
ability to aggregate together capabilities in a dictionary while anonymizing the
capability names. Aggregating items into a dictionary in an anonymized fashion
will be supported for the two capability types capable of holding items:
dictionaries and directories.

When a dictionary is offered into a dictionary with the field
`aggregation_strategy` set to `flatten_and_anonymize`, the dictionary will be
watched and any entries that are added to or removed from the first dictionary
will be added to or removed from the second dictionary under anonymized names.

When a directory is offered into a dictionary with the field
`aggregation_strategy` set to `flatten_and_anonymize`, component manager will:

- Register a `fuchsia.io.DirectoryWatcher` on the directory.
- Watch for any service or directory nodes that are added or removed.
- When a new node is discovered, create new `Connector` and `DirConnector`
  capabilities which forward any open calls to the directory entry they
  correlate to.
- Add those `Connector` and `DirConnector` capabilities to the dictionary under
  anonymized names.
- When a node is removed, delete the respective capability from the dictionary.

The source of the dictionary or directory whose entries will be watched may come
from a collection, just like with anonymized service aggregates. When a
collection source is specified, any component in the collection will be checked
for an exposed directory or dictionary with the given name, and if such a
capability exists then it will also have its entries added to the aggregate.

```{
    capabilities: [
        {
           dictionary: "my_dictionary",
        }
    ],
    offer: [
        {
            dictionary: "child_dictionary",
            from: "#my_child",
            to: "self/my_dictionary",
            aggregation_strategy: "flatten_and_anonymize",
        },
        {
            directory: "parent_directory",
            from: "parent",
            to: "self/my_dictionary",
            aggregation_strategy: "flatten_and_anonymize",
        },
    ],
}
```

#### Example

Imagine a component that declares and exposes a directory capability. The
contents of this directory follow the expectations for a FIDL service, in that
the directory contains directories which themselves contain service nodes.

```
{
    program: {
        runner: "elf",
        binary: "bin/app",
    },
    capabilities: [ {
        directory: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        rights: "r*",
    } ],
    expose: [ {
        directory: "fuchsia.example.service.Echo",
        from: "self",
    } ],
}
```

Imagine now that two instances of this component are launched in a collection,
the parent merges their directory capabilities into a dictionary with
`flatten_and_anonymize`, and then uses that dictionary.

```
{
    program: {
        runner: "elf",
        binary: "bin/app",
    },
    collections: [ {
        name: "my_collection",
    } ],
    capabilities: [ {
       dictionary: "fuchsia.example.service.Echo",
    } ],
    offer: [ {
        directory: "fuchsia.example.service.Echo",
        from: "#my_collection",
        to: "self/fuchsia.example.service.Echo",
        aggregation_strategy: "flatten_and_anonymize",
    } ],
    use: [ {
        dictionary: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        from: "self",
    } ],
}
```

In this setup, any directory entries that the children in the collection publish
inside of their `fuchsia.example.service.Echo` directory will appear as entries
in `my_dictionary` under random names, and thus also appear in the namespace of
the parent component (because of the `use` statement).

```
$ ffx component explore example_component/my_collection:child_a
$ ls out/svc/fuchsia.example.service.Echo/
default
other_instance
```

```
$ ffx component explore example_component/my_collection:child_b
$ ls out/svc/fuchsia.example.service.Echo/
default
other_instance
```

```
$ ffx component explore example_component
$ ls ns/svc/fuchsia.example.service.Echo/
5ddc3df4dd9e981b9fef1d8164066fa2 # open requests to this go to `default` in `child_a`
6dfabc3e1a9585fef840d458f5cc9ba5 # open requests to this go to `other_instance` in `child_a`
72c86121a7696881f86d855170fd55cd # open requests to this go to `default` in `child_b`
819be91c4070d1b009c6b61b6e4dd422 # open requests to this go to `other_instance` in `child_b`
```

### Filtered aggregation of dictionaries

To mirror the "filter/rename aggregate" feature of services, we will add the
ability to aggregate together capabilities in a dictionary based on renaming
rules that dictate which entries from the source will be added to the dictionary
and under which names. As with anonymized aggregates, both dictionaries and
directories may be sources for a filter/rename aggregate.

These renaming rules will take the form of two new fields on directory and
dictionary offers: `renames` and `filter`. These fields will respectively hold
name mappings between source and target entries, and the set of source entries
that should be made present in the target. A remap where the source and target
names are equal is functionally equivalent to a filter with the same name.

When a dictionary is offered into a dictionary with the field
`aggregation_strategy` set to `flatten_and_rename`, entries from the former will
be added to the following following the renaming and filtering set in the offer.

When a directory is offered into a dictionary with the field
`aggregation_strategy` set to `flatten_and_rename`, component manager will:

- Create a new `DirConnector` for each rename or filter rule, which will forward
  any open call into the directory entry they correlate with in the backing
  directory.
- Add the `DirConnector` under the target name to the dictionary.

```
{
    capabilities: [
        {
            dictionary: "my_dictionary",
        }
    ],
    offer: [
        {
            dictionary: "child_dictionary",
            from: "#my_child",
            to: "self/my_dictionary",
            aggregation_strategy: "flatten_and_rename",
            renames: {
                foo: "bar",
                baz: "baz",
            },
        },
        {
            directory: "parent_directory",
            from: "parent",
            to: "self/my_dictionary",
            aggregation_strategy: "flatten_and_rename",
            renames: {
                a: "b",
            },
            filter: [
              "c",
            ],
        },
    ],
}
```

#### Example

Imagine a component that declares and exposes a directory capability. The
contents of this directory follow the expectations for a FIDL service, in that
the directory contains directories which themselves contain service nodes.

```
{
    program: {
        runner: "elf",
        binary: "bin/app",
    },
    capabilities: [ {
        directory: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        rights: "r*",
    } ],
    expose: [ {
        directory: "fuchsia.example.service.Echo",
        from: "self",
    } ],
}
```

Imagine now that two instances of this component are included as static children
of a different component. The parent merges their directory capabilities into a
dictionary with `flatten_and_rename`, and then uses that dictionary.

```
{
    program: {
        runner: "elf",
        binary: "bin/app",
    },
    children: [
        {
            name: "child_a",
            url: "fuchsia-pkg://...",
        },
        {
            name: "child_b",
            url: "fuchsia-pkg://...",
        },
    ],
    capabilities: [ {
       dictionary: "fuchsia.example.service.Echo",
    } ],
    offer: [
        {
            directory: "fuchsia.example.service.Echo",
            from: "#child_a",
            to: "self/fuchsia.example.service.Echo",
            aggregation_strategy: "flatten_and_rename",
            renames: {
                default: "a",
                other_instance: "b",
            },
        },
        {
            directory: "fuchsia.example.service.Echo",
            from: "#child_b",
            to: "self/fuchsia.example.service.Echo",
            aggregation_strategy: "flatten_and_rename",
            renames: {
                default: "c",
                other_instance: "d",
            },
        },
    ],
    use: [ {
        dictionary: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        from: "self",
    } ],
}
```

In this setup, the dictionary used by the parent will contain entries `a`, `b`,
`c`, and `d`. When the parent opens any of these four entries, the open request
is forwarded to a directory exposed by one of the children.

```
$ ffx component explore example_component/child_a
$ ls out/svc/fuchsia.example.service.Echo/
default
other_instance
```

```
$ ffx component explore example_component/child_b
$ ls out/svc/fuchsia.example.service.Echo/
default
other_instance
```

```
$ ffx component explore example_component
$ ls ns/svc/fuchsia.example.service.Echo/
a # open requests to this go to `default` in `child_a`
b # open requests to this go to `other_instance` in `child_a`
c # open requests to this go to `default` in `child_b`
d # open requests to this go to `other_instance` in child_b
```

### Providers of FIDL services

Any component that today is providing a FIDL service to other components will
declare and route a read-only directory, instead of a service.

This:

```
{
    capabilities: [ {
        service: "fuchsia.example.service.Echo",
    } ],
    expose: [ {
        service: "fuchsia.example.service.Echo",
        from: "self",
    } ],
}
```

Becomes this:

```
{
    capabilities: [ {
        directory: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        rights: "r*",
    } ],
    expose: [ {
        directory: "fuchsia.example.service.Echo",
        from: "self",
    } ],
}
```

### Consumers of FIDL services

Any component that today is consuming a FIDL service will instead either use a
directory or a dictionary, depending on whether it is directly consuming a
component-provided FIDL service or if it is consuming a component manager hosted
service aggregate.

This:

```
{
    use: [ {
        service: "fuchsia.example.service.Echo",
    } ],
}
```

Becomes this:

```
{
    use: [ {
        directory: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        rights: "r*",
    } ],
}
```

Or this:

```
{
    use: [ {
        dictionary: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
    } ],
}
```

### Reducing boilerplate

The design proposed here should be more flexible and more powerful than
services, unlocking new use cases and making the system more understandable.
However, the transformations shown in the examples above result in more
boilerplate (and in some scenarios excessively more boilerplate). To not
sacrifice succinctness in the name of flexibility, some new syntax sugar will be
added to CML to make the most common tasks roughly as verbose as before this
change.

#### Declaring and using service-compatible capabilities

Directories or dictionaries do not have default rights or paths, whereas
services had default paths and were assumed to always have `r*` rights.

In scenarios where a directory or dictionary is intended to be used to hold
instances of protocols or proper FIDL service instances, it can be referred to
as a `service_directory` or `service_dictionary` to use the same defaults that
services had.

For example, the following two use declarations are identical:

```
{
    use: [ {
        directory: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        rights: "r*",
    } ],
}
```

```
{
    use: [ {
        service_directory: "fuchsia.example.service.Echo",
    } ],
}
```

The following two capability declarations are also identical:

```
{
    capabilities: [ {
        directory: "fuchsia.example.service.Echo",
        path: "/svc/fuchsia.example.service.Echo",
        rights: "r*",
    } ],
}
```

```
{
    capabilities: [ {
        service_directory: "fuchsia.example.service.Echo",
    } ],
}
```

#### Routing from multiple components at once

Service capability routes may have a source of a collection (which holds 0 or
more components) and/or multiple sources. In these scenarios an anonymous
aggregate is implicitly created and routed. To mirror this functionality, when a
dictionary is routed with multiple sources it will be assumed that the manifest
author wants to create an anonymous aggregate and route that instead.

For example, the following two manifest snippets are identical:

```
{
    collections: [ { name: "boot-drivers" } ],
    capabilities: [
        { dictionary: "fuchsia.hardware.usb.device.Service" },
    ],
    offer: [
        {
            directory: "fuchsia.hardware.usb.device.Service",
            from: "#boot-drivers",
            to: "self/fuchsia.hardware.usb.device.Service",
            aggregation_strategy: "flatten_and_anonymize",
        },
    ],
    expose: [ {
        dictionary: "fuchsia.hardware.usb.device.Service",
        from: "self",
    } ],
```

```
{
    collections: [ { name: "boot-drivers" } ],
    expose: [ {
        dictionary: "fuchsia.hardware.usb.device.Service",
        from: "#boot-drivers",
    } ],
}
```

### Making service publishing expectations predictable

Services whose instances are added to an anonymizing aggregate are subject to
unique behavior from component manager when it comes to expectations around how
those services are published in the component's outgoing directory.

Typically when a component declares that it can provide a capability, such as a
directory or protocol, it is expected to place the entries for those
capabilities in its outgoing directory before it begins accepting requests to
its outgoing directory. This is because component manager does not check in
advance if the entries in an outgoing directory exist, it simply sends an open
request to the path listed in the manifest and it's up to the component to
handle that correctly. If the component were to start processing those open
requests before its outgoing directory is properly initialized, it might
accidentally close those open requests and cause its client to see a peer
closed. This is generally an easy requirement for component authors to meet, as
commonly used library tooling such as fuchsia_component_server implements this
behavior correctly.

Service capabilities which contribute to an anonymizing aggregate do not have
this expectation. When component manager is routing the service capabilities
that make up the aggregate and it reaches the component providing such a
service, it will register a `fuchsia.io.DirectoryWatcher` on the root of the
outgoing directory, wait for the next item in the path to proceed, and then
perform the same operation on the next item and so forth until it has reached
the directory that the service capability is listed at in the manifest.

For example, imagine a component with this manifest:

```
{
    capabilities: [ {
        service: "fuchsia.example.service.Echo",
    } ],
}
```

The service capability should be placed in the outgoing directory at the path
`/svc/fuchsia.example.service.Echo`. Imagine that when the component starts this
is not the case, the outgoing directory is empty, and the component immediately
starts processing requests to its outgoing directory handle. Then 1 second after
starting, the `svc` directory is added to the outgoing directory, and after an
additional second the `fuchsia.example.service.Echo` directory is added to that.

If this service capability were not part of an aggregate and directly consumed
by another component, the client component may see a spurious "peer closed"
message because the open request to the capability provider would be rejected as
the outgoing directory does not have any entry at the path of the service
capability.

If this service capability were part of an anonymizing aggregate and that
aggregate was consumed by another component, the client component will never see
a spurious "peer closed" message, and 2 seconds after the provider has started
any service instances it publishes will be visible by the client.

This behavior is special, and deviates from typical component expectations.
Having such a special edge case complicates the system and makes it less
predictable, so the aggregate operations for dictionaries will not mirror this
behavior for directory sources by default.

For backwards compatibility, a flag will be added named
`support_slow_publishing` that will enable this watcher-based behavior when
opening a directory that contributes entries to a dictionary aggregate.

```
{
   offer: [
        {
            directory: "parent_directory",
            from: "parent",
            to: "self/my_dictionary",
            aggregation_strategy: "flatten_and_anonymize",
            support_slow_publishing: true,
        },
    ],
}
```

This flag may be deprecated at a future date, if components that depend on this
behavior are updated to no longer do so. Updating these components is out of
scope of this proposal, so there are no active plans to deprecate this at time
of writing.

## Benefits

### Single-protocol FIDL services can be flattened

#### Multiple instances per component

If a component author wishes to deviate from the FIDL services contract, instead
of a directory of directories of service nodes (i.e. a service capability of
service instances of service members) components could instead provide/consume
directories of protocols. In such a model components in a collection could
expose directories of service nodes, and rely on the `flatten_and_anonymize`
aggregation strategy to merge any service nodes in those directories into the
dictionary aggregate.

For example, a component which provides and exposes a directory of protocols:

```
{
    capabilities: [ {
        // holds service nodes that all implement this protocol
        directory: "fuchsia.examples.Echo",
        rights: "r*",
        path: "/svc/fuchsia.examples.Echo",
    } ],
    expose: [ {
        directory: "fuchsia.examples.Echo",
        from: "self",
    } ],
}
```

Could be added to a collection, and have those protocols found and added to an
aggregate:

```
{
    offer: [ {
        directory: "fuchsia.examples.Echo",
        from: "#my_collection",
        to: "self/my_dictionary",
        aggregation_strategy: "flatten_and_anonymize",
    } ],
}
```

`my_dictionary` will thus hold anonymized protocol instances published by
components in `my_collection`, where each component may publish 0 or more
instances of the protocol. Using `my_dictionary` like this:

```
{
    use: [ {
        dictionary: "my_dictionary",
        path: "/svc/fuchsia.examples.Echo",
    } ],
}
```

will result in protocol nodes being available in the using component's namespace
at the path `/svc/fuchsia.examples.Echo/<randomized name>`.

#### One instance per component

With the above design, it would be easy to add "non-flattening aggregation",
such that instead of searching for entries within directory or dictionary
capabilities included in the bundle, the capabilities included in the bundle are
directly added under randomized names.

With this addition, it would be easy to do things like create dynamically
updated dictionaries of every protocol under a certain name exposed from a
collection.

```
{
   offer: [
        {
            protocol: "fuchsia.examples.Echo",
            from: "#my_collection",
            to: "self/my_dictionary",
            aggregation_strategy: "anonymize",
        },
    ],
}
```

In the above example, any component that uses `my_dictionary` will see a
directory with service nodes under randomized names, with each service node
correlating to one fuchsia.examples.Echo protocol exposed by a component in
`my_collection`.

### Service publishing semantics are more predictable

With service capabilities today, at the point the service capability is used
it's not possible to tell if the service capability is an aggregate or if the
service is directly provided by another component. This creates friction, as in
some cases this distinction is very important!

Imagine that component A publishes service instances consumed by component B,
and they communicate about these service instances over a separate FIDL
protocol. If component A publishes a new service instance and then immediately
tells B about this, B can open the service instance as soon as it's been told by
A about it. There's no chance for a race because A can guarantee that the
service instance is published before sending B the message about it.

Now imagine instead that A's service capability is fed into an anonymizing
aggregate that B consumes. When A publishes a service instance and then tells B
about it, there's no guarantee that B will immediately be able to open the
service instance. This is because component manager's directory watcher first
needs to notice the new instance and add the appropriate node to its service
VFS. These two operations, B's open request and component manager's watcher
logic, can race.

With the design proposed here, a component that uses a FIDL service will always
know if it's consuming a direct connection to another component, or a VFS hosted
by component manager, based on if it's using a directory or a dictionary.

### Aggregation strategy is explicit

The service aggregation strategy is keyed on one thing: whether or not the
`renames` and/or `filter` fields are set. This makes for a rather unintuitive
experience, as it's not plainly clear (without reading documentation) what
happens if the `renames` field is not set. If there are no renames or filter,
it's possible to guess that this would result in a service capability with no
service instances (and perhaps should be a validation error), instead of an
entirely different aggregation strategy being engaged in this scenario.

Now we can emit an informative validation error message if the rename strategy
is selected without any rename or filtering rules, and it's clear when the
anonymizing strategy is followed instead.

### Individual capabilities can be removed from bundles and routed

Capabilities inside of dictionaries may be taken from the dictionary and routed
individually. This will allow routing individual service instances out of
dictionaries, something that is impossible to do with services.

```
{
   offer: [
        {
            directory: "default",
            as: "fuchsia.examples.services.Echo_default",
            from: "parent/fuchsia.examples.services.Echo",
            to: "#my_child",
        },
    ],
}
```

## Implementation

### Step 1: auto conversion during routing

We will temporarily relax capability type checks such that if one component
exposes or offers a service capability, it may be referred to and exercised as a
dictionary or as a directory.

For example:

```
{
    children: [ { name: "my_child", url: "..." } ],
    // Offers service to child
    offer: [ { service: "foo", from: "parent", to: "#my_child" }],
}
```

```
{
    // Parent offered a service capability, but we can "use" it as a
    // dictionary. Namespace is identical to if we "used" it as a
    // service.
    use: [ { dictionary: "foo" } ],
}
```

As another example:

```
{
    capabilities: [ { service: "foo" } ],
    // Exposes service to parent
    expose: [ { service: "foo", from: "self" } ],
}
```

```
{
    capabilities: [
        {
            dictionary: "my_dictionary",
        }
    ],
    children: [ { name: "my_child", url: "..." } ],
    offer: [
        {
            // Child exposed a service capability, but we can aggregate
            // it into "my_dictionary" as a dictionary
            dictionary: "foo",
            from: "#my_child",
            to: "self/my_dictionary",
            aggregation_strategy: "anonymized",
        },
    ],
}
```

### Step 2: component manifest migrations

#### Step 2.1: static manifests

Some CML sections will be rewritten from services to dictionaries, while others
will be rewritten from services to directories. It is not necessarily possible
to look at a component manifest in isolation and determine which rewrite is
appropriate, as it depends on the source of the capability route. If the
service's source is an aggregate, then it should be a dictionary. If the
service's source is a component, then it should be a directory.

Because of the above, we cannot write a tool that takes in a CML file and spits
out a modified CML file, as the modifications depend on the routing source.
Instead, we will rely on the engine we have to determine what a capability's
source is: component manager itself. Patches will be written for component
manager (and not merged) to add the ability to ask it to identify every
component that mentions a service in its manifest, find the source of that
service, and then output recommendations for the necessary changes to that
component's manifest.

Given the list of recommendations from component manager, a human will find the
CML files that correlate with each recommendation and make the necessary changes
by hand, working from target to source along each route. This will make it easy
to identify and update the majority of service usages across various products,
with the remaining being far more tractable to hunt down and update by hand.

This work will be owned by the component framework, component authors are merely
asked to review and then approve the changes.

#### Step 2: runtime manifest generation

Some components interact with component manifests programmatically, and thus
rely on the service capability pieces of `fuchsia.component.decl`. These
components fall broadly into two camps: test code (often using realm builder),
and dynamic offers.

Part of the component manifest migration will require refactoring these
components to instead generate dictionary capabilities.

Test code, especially code that relies on realm builder, will be easy to migrate
as hermetic tests include all of their components in the test environment. An
entire test's worth of components can thus be migrated atomically.

Components which generate dynamic offers for service capabilities can be
migrated once the downstream components of them have been migrated. Dynamic
offers which do not involve any aggregation will be easy to migrate, as much
like with CML the service offer can simply be swapped out for a directory offer.

The more complex scenario is dynamic offers which do involve aggregation. If a
dynamic offer for a service specifies rename rules and/or conflicts with another
offer for a service by the same name, then service aggregation is performed.

To meet this requirement, dictionary and directory offers may specify an
`aggregation_strategy` when used as a dynamic offer. This will cause an
anonymous dictionary to be created, which will be populated according to the
chosen aggregation strategy and then placed in the inputs to the child
component.

#### Step 3: service deprecation

Once most or all known service usages have been migrated, services are no longer
recommended. We will modify component manager to log a warning whenever a
component is resolved that mentions a service in its manifest. Service
capabilities will be marked as deprecated in fuchsia.component.decl in the next
API level.

#### Step 4: service deletion

After the API version that deprecates services has been branched, services are
no longer allowed. The log message from step 3 will be upgraded to an error
level log, and service capabilities will be removed from both CML and
`fuchsia.component.decl` in the next API level.

## Performance

There should be no performance impact. Any components which do not have
component manager sitting in the middle of their service route will continue to
have a direct connection to each other. Any components which do have component
manager sitting in the middle will remain in an identical situation from a
performance perspective.

## Ergonomics

This change reduces the number of concepts in the component framework at the
FIDL and CML levels, which will make the system both easier to learn and
modify.

## Backwards Compatibility

No components which rely on service capabilities in their component manifests
will continue to work past the soft migration horizon in step 4 of the
implementation plan.

## Security considerations

Before this change the contents of a dictionary were always statically
determined, with the sole exception of the "dynamic dictionary" feature whose
usage is limited with an allowlist.

The proposal here introduces another mechanism by which the contents of a
dictionary may vary, wherein one or more components may publish new protocols or
directories at runtime for inclusion in a given dictionary (in a services
context, these are the service instances).

To help ensure that auditing Fuchsia is not made more difficult by the
introduction of this added dynamism, the following steps will be taken.

### The security team retains decision-making authority over the allowlist for
### using dictionaries

Today using a dictionary requires an entry on an allowlist owned by the security
team. This proposal will significantly increase the number of components that
wish to do this. It will remain solely at the security team's discretion when
and whether or not they wish to relax this allowlist, the entries on it, and so
forth.

### A new tool is introduced: ffx component sandbox

Recent refactors in component manager have resulted in the concept of a
component's sandbox. This holds the complete set of capabilities that are
relevant for a component, including things like the capabilities offered to it,
the framework capabilities it could use/offer/expose, and the set of
capabilities that map to entries in the component's outgoing directory. A
component's sandbox is comprised of a set of dictionaries.

A new tool will be introduced named `ffx component sandbox` that will print out
the contents of these dictionaries as well as the sources of anything in them
(in a recursive fashion, so the contents of an offered dictionary are also
displayed).

The exact formatting of this tool will be determined at implementation time, but
it will display the following (along with their sources, and the contents of any
dictionaries within these sets and the sources of those contents):

- All capabilities offered to the component
- All capabilities in the component's environment
- All capabilities the component exposes
- All capabilities used by the component
- All capabilities the component declares in its manifest
- All capabilities the component can use `from: "framework"`
- All capabilities the component offers to each child/collection


Notably this tool will be built in such a
way that it can both be run on a host machine (relying on the scrutiny stack)
and against a target machine. This approach will allow the tool to be used to
inspect user builds of Fuchsia, which to date have been complicated to audit due
to the limitations around `ffx` tooling on such builds.

This should be a powerful addition to the auditing arsenal available to anyone
who wishes to gain an understanding of the system. Any questions around what
things a component could possibly access or make available to its neighbors
will become easy to answer, including the scope of things that may be in routed
dictionaries.

## Privacy considerations

This change has no impact on user data, and thus no impact on privacy.

## Testing

Services today have ample test coverage of both unit and integration tests
within the component framework. These tests will be duplicated and modified to
cover capability aggregation with dictionaries in tandem with the development of
these dictionary aggregation features.

## Documentation

Documentation available on [fuchsia.dev][fuchsia-dev] will be updated to reflect
the new dictionary features, best practices around using them, and the
deprecation of service capabilities. Example usage of the new dictionary
features will be added to `//examples/components`.

## Drawbacks, alternatives, and unknowns

This design makes an intentional decision to make it abundantly clear when a
component is connected directly to another component versus when component
manager is hosting a VFS that sits between two components. If a component uses a
directory, that will be provided by another component. If a component uses a
dictionary, that will be a VFS hosted by component manager.

This clear distinction aims to address something that's been a pain point in
some specific scenarios, but it's possible that requiring component authors to
always know if they need to use a directory or dictionary will become a new pain
point.

If this is identified as overly frustrating for users, we will evolve best
practices to recommend using dictionaries exclusively outside of situations
where it truly is important for two components to have a direct connection to
each other.

[services-rfc]: /docs/contribute/governance/rfcs/0041_unifying_services_devices.md
[dictionaries-rfc]: /docs/contribute/governance/rfcs/0235_component_dictionaries.md
[fuchsia-dev]: https://fuchsia.dev
