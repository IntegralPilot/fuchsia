# Dictionary capabilities

[Dictionaries][glossary-dictionary] allow multiple capabilities to be grouped as
a single unit and routed together as a single capability.

The format of a dictionary is a key-value store, where the key is a
[capability name][capability-names] string and the value is a capability. The
value itself may be another dictionary capability, which can be used to achieve
directory-like nesting.

## Defining dictionary capabilities {#define}

To define a new dictionary, you add a `capability` declaration for it like so:

```json5
capabilities: [
    {
        dictionary: "bundle",
    },
],
```

Read the section on [aggregation](#aggregation) to learn how to add capabilities
to the dictionary.

From the outset, there is an important distinction to call out between
dictionary capabilities and most other capability types in the component
framework like [protocols][capability-protocol] and
[directories][capability-directory]. A protocol capability is ultimately hosted
by the program associated with the component; the `protocol` capability
declaration is just a way of making the component framework aware of that
protocol's existence. A `dictionary` capability, on the other hand, is always
hosted by the component framework runtime. In fact, a component does not need to
have a `program` to declare a `dictionary` capability.

## Routing dictionary capabilities {#route}

Because dictionaries are a collection type, they support a richer set of routing
operations than other capability types.

The basic operations to [expose](#expose) a dictionary to its parent or
[offer](#offer) it to a child are similar to other capabilities. But
dictionaries also support the following additional routing operations:

-   [Aggregation](#aggregation): Install capabilities into a dictionary defined
    by this component.
-   [Nesting](#nesting): Aggregate a dictionary inside a dictionary, and use
    path syntax to refer to capabilities within the inner dictionary.
-   [Retrieval](#retrieval): Retrieve a capability from a dictionary and route
    or use it independently.
-   [Extension](#extension): Define a dictionary that includes all the
    capabilities from another.

A dictionary cannot be modified after it is created. Routing a dictionary grants
only readable access to it. [Mutability](#mutability) explains the semantics of
mutability for dictionaries in more detail.

For more general information on capability routing, see the
[top-level page][capability-routing].

### Exposing {#expose}

Exposing a dictionary capability grants the component's parent access to it:

```json5
{
    expose: [
        {
            dictionary: "bundle",
            from: "self",
        },
    ],
}
```

Like other capabilities, you can change the name seen by the parent with the
`as` keyword:

```json5
{
    expose: [
        {
            dictionary: "local-bundle",
            from: "self",
            as: "bundle",
        },
    ],
}
```

### Offering {#offer}

Offering a dictionary capability grants a child component access to it:

```json5
{
    offer: [
        {
            dictionary: "bundle",
            from: "parent",
            to: [ "#child-a", "#child-b" ],
        },
    ],
}
```

Like other capabilities, you can change the name seen by the child with the `as`
keyword:

```json5
{
    offer: [
        {
            dictionary: "local-bundle",
            from: "self",
            to: [ "#child-a", "#child-b" ],
            as: "bundle",
        },
    ],
}
```

### Using {#use}

Currently the framework does not support `use`ing a `dictionary` capability as
such. However, individual capabilities may be [retrieved](#retrieval) from a
dictionary and used that way.

### Aggregation {#aggregation}

To add capabilities to a dictionary that you defined, you use the `offer`
keyword and specify the target dictionary in `to`. This operation is called
*aggregation*. The dictionary must be defined by the same component that
contains the `offer`.

To indicate that you wish to add a capability to the target dictionary, use the
following syntax in `to`:

```
to: "self/<dictionary-name>",
```

where there exists a `capabilities` declaration for `dictionary:
"<dictionary-name>"`. The `self/` prefix reflects the fact that the dictionary
is local to this component. (This is one case of the dictionary path syntax
described in [Retrieval](#retrieval).)

Just like other kinds of `offer`, the name of the capability used as the key in
the dictionary may be changed from its original name, using the `as` keyword.

Here is an example of aggregation at work:

```json5
capabilities: [
    {
        dictionary: "bundle",
    },
    {
        directory: "fonts",
        rights: [ "r*" ],
        path: "/fonts",
    },
],
offer: [
    {
        protocol: "fuchsia.examples.Echo",
        from: "#echo-server",
        to: "self/bundle",
    },
    {
        directory: "fonts",
        from: "self",
        to: "self/bundle",
        as: "custom-fonts",
    },
],
```

### Nesting {#nesting}

A special case of [aggregation](#aggregation) is *nesting*, where the capability
added to the dictionary is itself a dictionary. For example:

```json5
capabilities: [
    {
        dictionary: "bundle",
    },
],
offer: [
    {
        dictionary: "gfx",
        from: "parent",
        to: "self/bundle",
    },
],
```

In this way, it is possible to nest capabilities in a dictionary deeper than one
level. Read on to the next section for an illustration of this.

### Retrieval {#retrieval}

The act of accessing a capability from a dictionary to route it independently is
called *retrieval*.

There are two inputs to a retrieval operation: the dictionary to retrieve the
capability from, and the key of a capability in that dictionary. CML represents
those as follows:

-   The path to the dictionary is supplied in the `from` property.
    -   The syntax is: `"<source>/<path>/<to>/<dictionary>"`.
    -   `<source>` can be any of the usual sources that the routing operation
        supports. For example, `offer` supports `self`, `#<child>`, or `parent`,
        and `expose` supports `self` or `#<child>`.
    -   The remaining path segments identify a (possibly nested) dictionary
        routed to this component by `<source>`.
    -   A good way to understand this is as a generalization of the ordinary
        non-nested `from` syntax (as described for example
        [here][capability-offer]). Conceptually, you can think of the `<source>`
        as a special "top level" dictionary provided by the framework. For
        example, `parent` is the name of the dictionary containing all
        capabilities offered by the parent, `#<child>` is a dictionary
        containing all capabilities exposed by `<child>`, etc. If you extend the
        path with additional segments, it just indicates a dictionary that's
        nested deeper within the top level one.
-   The key naming the capability is supplied as the value of the capability
    keyword (`protocol`, `directory`, etc.) in the routing declaration. This is
    identical to the syntax for naming a capability that's not routed in a
    dictionary.

This syntax is easiest to illustrate by example:

```json5
expose: [
    {
        protocol: "fuchsia.examples.Echo",
        from: "#echo-realm/bundle",
        to: "parent",
    },
],
```

In this example, this component expects that a dictionary named `bundle` is
exposed by `#echo-realm`. `from` contains the path to this dictionary:
`#echo-realm/bundle`. The capability in the `bundle` dictionary that the
component wishes to retrieve and route is a `protocol` with the key
`fuchsia.examples.Echo`. Finally, this protocol will be exposed to the
component's parent as `fuchsia.examples.Echo` (as an individual capability, not
as part of any containing dictionary).

Similar syntax is compatible with `use`:

```json5
use: [
    {
        protocol: "fuchsia.examples.Echo",
        from: "parent/bundle",
    },
],
```

In this example, the component expects the parent to offer the dictionary
`bundle` containing the protocol `fuchsia.examples.Echo`. No `path` is
specified, so the path of the protocol in the program's incoming namespace will
be [the default][capability-protocol-consume]: `/svc/fuchsia.examples.Echo`.

Note that normally, the default value for `from` in use declarations is
`"parent"`, but because we are retrieving the protocol from a dictionary offered
by the parent, we have to specify the parent as the source explicitly.

Since dictionaries can be [nested](#nested), they themselves may be retrieved
from dictionaries and routed out of them:

```json5
offer: [
    {
        dictionary: "gfx",
        from: "parent/bundle",
        to: "#echo-child",
    },
],
```

In this example, the component assumes that the parent offers a dictionary to it
named `bundle`, and this `bundle` contains a dictionary named `gfx`, which is
offered to `#echo-child` independently.

`from` supports arbitrary levels of nesting. Here's a variation on the previous
example:

```json5
offer: [
    {
        protocol: "fuchsia.ui.Compositor",
        from: "parent/bundle/gfx",
        to: "#echo-child",
    },
],
```

Like the last example, the component assumes that the parent offers a dictionary
to it named `bundle`, and this `bundle` contains a dictionary named `gfx`.
Finally, `gfx` contains a protocol capability named `fuchsia.ui.Compositor`,
which is offered independently to `#echo-child`.

Finally, it is even possible to combine retrieval with
[aggregation](#aggregation), routing a capability from one dictionary to
another:

```json5
capabilities: [
    {
        dictionary: "my-bundle",
    },
],
offer: [
    {
        protocol: "fuchsia.examples.Echo",
        from: "parent/bundle",
        to: "self/my-bundle",
    },
],
```

### Extension {#extension}

In some cases, you might want to build a dictionary that contains capabilities
added by multiple components. You cannot do this by [aggregating](#aggregation)
a single dictionary across multiple components because a component is only
allowed to add capabilities to a dictionary that it [defined](#define).

However, you can accomplish something similar via *extension*. The extension
operation allows you to declare a new dictionary whose initial contents are
copied from another dictionary (called the "source dictionary"). In this way,
you can create a new dictionary that incrementally builds upon a previous one
without having to individually route all the capabilities from it.

Normally, when you extend a dictionary, you will want to add additional
capabilities to the extending dictionary. All keys used for the additional
capabilities must not collide with any keys from the source dictionary. If they
do, it will cause a routing error at runtime when someone tries to retrieve a
capability from the extending dictionary.

You declare a dictionary as an extension of another by adding the `extends`
keyword to the dictionary's `capability` declaration that identifies the source
dictionary. The syntax for `extends` is the same as that for `from` discussed in
[Retrieval](#retrieval).

For example:

```json5
capabilities: [
    {
        dictionary: "my-bundle",
        extends: "parent/bundle",
    },
],
offer: [
    {
        protocol: "fuchsia.examples.Echo",
        from: "#echo-server",
        to: "self/my-bundle",
    },
    {
        dictionary: "my-bundle",
        from: "self",
        to: "#echo-client",
        as: "bundle",
    },
],
```

As with `from`, the path in `extends` can refer to a nested dictionary:

```json5
capabilities: [
    {
        dictionary: "my-gfx",
        extends: "parent/bundle/gfx",
    },
],
```

#### Dynamic dictionaries {#dynamic}

There is another kind of `dictionary` capability where the dictionary is
created by the component's program itself at runtime. These dictionaries do
not support [extension](#extension) or [aggregation](#aggregation) in CML;
it is up to the program to populate the dictionary using the fidl
[sandbox][fidl-sandbox-capability-store] API.

```json5
capabilities: [
    {
        dictionary: "my-dynamic-dictionary",
        path: "<outgoing-dir-path>",
    },
],
```

`<outgoing-dir-path>` is a path in the component's
[outgoing directory][glossary-outgoing-directory] to a
[`fuchsia.component.sandbox/DictionaryRouter`][fidl-sandbox-router] protocol which is
expected to return a [`Dictionary`][fidl-sandbox-dictionary] capability.

To illustrate this feature, let's walk through an example. The example consists
of two components.

-   `dynamic-dictionary-provider`: Creates a runtime
    [`Dictionary`][fidl-sandbox-dictionary] with three `Echo` protocol
    instances. It exposes the `Dictionary` via a [`DictionaryRouter`][fidl-sandbox-router]
    and defines a `dictionary` backed by it.
-   `dynamic-dictionary`: Declares CML to [retrieve](#retrieval) the three
    `Echo` protocols from the `dictionary`, and runs code to use each of these
    protocols.

##### Provider

The component manifest of `dynamic-dictionary-provider` is as follows. In it we
see the `dictionary` definition for `bundle` that names a `DictionaryRouter` in its
`path`.

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/meta/dynamic_dictionary_provider.cml" region_tag="body" adjust_indentation="auto" %}
```

At initialization, `dynamic-dictionary-provider` uses the
[`CapabilityStore`][fidl-sandbox-capability-store] sandbox API to create a new
`Dictionary` and adds three [`Connector`][fidl-sandbox-connector] to it. Each
`Connector` represents one of the `Echo` protocol instances.

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/provider/src/main.rs" region_tag="init" adjust_indentation="auto" %}
```

Each `Connector` is bound to a [`Receiver`][fidl-sandbox-receiver] which handles
incoming requests for `Echo`. The implementation of the `Receiver` handler is
very similar to a [`ServiceFs`][src-service-fs] handler, except unlike
`ServiceFs`, the `Receiver` is bound to the `Connector` instead of the
component's outgoing directory.

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/provider/src/main.rs" region_tag="receiver" adjust_indentation="auto" %}
```

Finally, we need to expose the dictionary created earlier with a
[`DictionaryRouter`][fidl-sandbox-router].

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/provider/src/main.rs" region_tag="serve" adjust_indentation="auto" %}
```

The [`DictionaryRouter`][fidl-sandbox-router] request handler exports the dictionary and
returns it. Note that it makes a
[`CapabilityStore.Duplicate`][fidl-sandbox-capability-store] of the dictionary
first because the framework may call `DictionaryRouter.Route` multiple times. This has the
effect of duplicating the dictionary *handle*, without copying the inner
contents.

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/provider/src/main.rs" region_tag="request" adjust_indentation="auto" %}
```

##### Client

The client side is simple. First, the component manifest retrieves the three
protocols from the `bundle` dictionary, using the normal [retrieval](#retrieval)
syntax.

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/meta/dynamic_dictionary.cml" region_tag="retrieval" adjust_indentation="auto" %}
```

The program just connects to each protocol in turn and tries to use it:

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/components/dictionaries/src/lib.rs" region_tag="connect" adjust_indentation="auto" %}
```

## Mutability

`dictionary` capabilities obey the following mutability rules:

-   Only the component that [defines](#define) a dictionary is allowed to
    [aggregate](#aggregation) or [extend](#extend) it. Furthermore, this is the
    only way to add capabilities to a `dictionary`, and there is no way to
    modify it at runtime.
-   Any component that is routed a dictionary can [retrieve](#retrieval)
    capabilities from it.
-   If a component makes two consecutive routing requests that attempt to
    retrieve from the same `dictionary`, it's possible that each request will
    return a different result. For example, one request might succeed while the
    other fails. This is a side effect of the on-demand nature of capability
    routing. In between both requests, it is possible that one of the components
    upstream [re-resolved][component-resolution] and changed the definition of
    its dictionary.

[component-resolution]: /docs/concepts/components/v2/lifecycle.md#resolving
[capability-directory]: ./directory.md
[capability-names]: https://fuchsia.dev/reference/cml#names
[capability-offer]: https://fuchsia.dev/reference/cml#offer
[capability-protocol]: ./protocol.md
[capability-protocol-consume]: ./protocol.md#consume
[capability-routing]: /docs/concepts/components/v2/capabilities/README.md#routing
[fidl-sandbox-capability-store]: https://fuchsia.dev/reference/fidl/fuchsia.component.sandbox#CapabilityStore
[fidl-sandbox-connector]: https://fuchsia.dev/reference/fidl/fuchsia.component.sandbox#Connector
[fidl-sandbox-dictionary]: https://fuchsia.dev/reference/fidl/fuchsia.component.sandbox#DictionaryRef
[fidl-sandbox-router]: https://fuchsia.dev/reference/fidl/fuchsia.component.sandbox#DictionaryRouter
[fidl-sandbox-receiver]: https://fuchsia.dev/reference/fidl/fuchsia.component.sandbox#Receiver
[glossary-dictionary]: /docs/glossary/README.md#dictionary
[glossary-outgoing-directory]: /docs/glossary/README.md#outgoing-directory
[src-service-fs]: /src/lib/fuchsia-component/server/src/lib.rs
