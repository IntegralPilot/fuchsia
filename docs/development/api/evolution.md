# Fuchsia Platform API evolution guidelines

This section contains guidelines for Fuchsia contributors making changes to
Fuchsia Platform APIs. Before you begin, you should be familiarized with the
following concepts:

- [FIDL versioning](/docs/reference/fidl/language/versioning.md)

## The lifecycle of a platform API {#lifecycle}

Fuchsia platform APIs should follow the lifecycle:
_Added → Deprecated → Removed → Deleted_, as illustrated below:

![This image shows the lifecycle of an API on the Fuchsia platform starting
  with the version when the API was added, then deprecated, then removed, and
  finally deprecated](images/platform-api-lifecycle.png "Fuchsia platform API
  lifecycle")

The following sections explain how to manage this lifecycle as an API developer.

### Adding FIDL APIs {#adding}

Always annotate new FIDL libraries and elements with an
[`@available`](/docs/reference/fidl/language/versioning.md) attribute. Unstable
APIs should be added at the `HEAD` API level. Note that partners using the SDK
_can_ target the `HEAD` API level, but get no API/ABI compatibility guarantees
if they do so.

For example:

```fidl
@available(added=HEAD)
library fuchsia.examples.docs;
```

When APIs are ready to stabilize, you should update them to be `added` at
`NEXT`. For example:

```fidl
@available(added=NEXT)
library fuchsia.examples.docs;
```

`NEXT` is similar to `HEAD`, except API elements available in `NEXT` will be
automatically added to the next published API level. So, if you added our
library to `NEXT` a week before the API freeze for F23, a week later you'd see:

```fidl
@available(added=23)
library fuchsia.examples.docs;
```

After that point, any API elements added to 23 will need to be supported until
all API levels that include those API elements have been retired.

When a FIDL library has more than one `.fidl` file, the library should include a
separate `overview.fidl` file and the `@available` attribute should be written
in that file along with a documentation comment describing the library. See
[the FIDL style guide][fidl-library-style]
for more information.

Every FIDL API in an [SDK category][sdk-category]
is opted into static compatibility testing in CI/CQ. These tests fail when
an API changes in backward incompatible ways. New libraries do not specify an
SDK category, preventing them from being exposed to partners, excluding them
from compatibility tests, and allowing the API to change freely. Once the API is
stable and the library has gone through [API calibration][calibration], specify
the `"partner"` category. See [Promoting an API][sdk-category-promotion] for
more information.

### Replacing FIDL APIs {#replacing}

Sometimes you need to replace an API with a new definition. To do this at API
level `N`, annotate the old definition with `@available(replaced=N)` and the new
definition with `@available(added=N)`. For example, this is how you would change
the value of a constant at API level 5:

```fidl
@available(replaced=5)
const MAX_LENGTH uint32 = 16;
@available(added=5)
const MAX_LENGTH uint32 = 32;
```

### Deprecating FIDL APIs {#deprecating}

It is important that the Fuchsia platform provides smooth transitions for
developers relying on the platform APIs. Part of that is providing ample warning
that APIs will be removed in the future.
[Deprecation](/docs/reference/fidl/language/versioning.md#deprecation) is one
way the this is communicated to developers.

You should always deprecate an API at an earlier level than you remove it. When
an end developer targets a deprecated API, they see a warning at build time that
the API is deprecated and they should migrate to an alternative. You should
include a note to help the end developer find an alternative. For example:

```fidl
protocol Example {
    // (Description of the method.)
    //
    // # Deprecation
    //
    // (Detailed explanation of why the method is deprecated, the timeline for
    // removing it, and what should be used instead.)
    @available(deprecated=5, removed=6, note="use Replacement")
    Deprecated();

    @available(added=5)
    Replacement();
};
```

There must be at least one API level between an API's deprecation and removal.
For example:

```fidl
// These are OK.
@available(deprecated=5, removed=6)
@available(deprecated=5, removed=100)
@available(added=5, deprecated=5)

// These will not compile.
@available(deprecated=5, removed=5)
@available(deprecated=5, removed=3)
```

### Removing FIDL APIs {#removing}

Note that you should always [deprecate](#deprecating) an API before removing it.

To remove an API that was added in a stable level, use the `@available`
attribute's `removed` argument. For example, to remove a method at level 18 that
was deprecated at level 12:

```
protocol Example {
    @available(added=10, deprecated=12, removed=18, note="Use Go() instead")
    Run() -> ();
};
```

In this example, an end developer targeting any level between 10 and 17
(inclusive) would see client bindings for the `Run` method, but a developer
targeting level 18 or greater would not. Developers working on the Fuchsia
platform would see bindings for the `Run` method as long as the platform
supports any level between 10 and 17, since the platform build targets the set
of all supported API levels.

Once the platform drops support for all levels between 10 and 17, you can delete
the `Run` method from the FIDL file if you wish. If you delete it before that
happens, static compatibility tests will fail and special approval from
//sdk/history/OWNERS will be required to submit the change.

To remove an API that was added at an unstable level such as `NEXT` or `HEAD`,
you can simply delete it from the FIDL file.

## Designing APIs that evolve gracefully {#evolve-gracefully}

This rubric focuses on promoting compatibility with a range of platform
versions. These attributes make compatibility as easy as possible to maintain
and is a subset of the [FIDL API Rubric][fidl-rubric].

### Follow the FIDL Style Guide

The [FIDL style guidelines][fidl-style] are used to make FIDL readable and
embody best practices. These are generally best practices, and should be
followed regardless of sdk_category.

### Use FIDL Versioning annotations

The [FIDL versioning annotations][fidl-versioning] allow libraries, protocols,
and other elements to be associated with specific API levels. All compatibility
reasoning is based on API version. This is how to express a point in the
evolution of an API.

- Only ever modify an API at the `NEXT` or `HEAD` API level.

- Changes should be implemented at `NEXT` only if they're ready to be released
  in the next Fuchsia milestone.

- Numbered API levels should not be changed. See
  [version_history.json][version-json].

### Specify bounds for vector and string

More information:
[FIDL API Rubric - Specify bounds for vector and string][fidl-string]

### Use enum vs. boolean

Since booleans are binary, the use of enum which can have multiple states is
preferred when making APIs compatibility-friendly. This way if an additional
state is needed, the enum can be extended, whereas the boolean would have to be
replaced with another type. More information:
[FIDL API Rubric - Avoid booleans if more states are possible][fidl-bool].

### Use flexible enums and bits

Flexible enums have a default unknown member, so it allows for easy evolution
of the enum.

Only use `strict` `enum` and `bits` types when you are _extremely_ confident
they will never be extended. `strict` `enum` and `bits` types cannot be
extended, and migrating them to `flexible` requires a migration for every field
with the given type.

More information: [FIDL Language - Strict vs. Flexible][fidl-enum]

### Prefer tables over structs

Both structs and tables represent an object with multiple named fields. The
difference is that structs have a fixed layout in the wire format, which means
they _cannot_ be modified without breaking binary compatibility. By contrast,
tables have a flexible layout in the wire format, which means fields _can_ be
added to a table over time without breaking binary compatibility.

More information:
[FIDL API Rubric - Should I use a struct or table?][fidl-table]

### Use open protocols with flexible methods and events

In general, all protocols should be `open`, and all methods and events within
those protocols should be `flexible`.

Marking a protocol as open makes it easier to deal with removing methods or
events when different components might have been built at different versions,
such that each component has a different view of which methods and events exist.
Because flexibility for evolving protocols is generally desirable, it is
recommended to choose open for protocols unless there is a reason to choose a
more closed protocol.

One potential exception is for _tear-off protocols_, representing a transaction,
where the only two-way method is a commit operation which must be strict while
other operations on the transaction may evolve. If a protocol is very small,
unlikely to change, and expected to be implemented by clients, you can make it
`closed` and all the methods `strict`. This will spare the client the trouble of
deciding how to handle an "unknown interaction." The cost, however, is that
methods or events can never be added to or removed from such a protocol. If you
decide you _do_ want to add a method or event, you'll need to define a new
tear-off protocol to replace it.

More information:

- [FIDL API Rubric - strict or flexible?][fidl-flexible]
- [FIDL API Rubric - Open, ajar, or closed?][fidl-open]

### Use the error syntax

The [error syntax][fidl-error-syntax] is used to specify a method will return a
value, or error out and return an int or enum representing the error.

## Use a custom error enum, not zx.Status

Use a purpose built enum error type when you define and control the domain. For
example, define an enum when the protocol is purpose built, and conveying the
semantics of the error is the only design constraint.

Use a domain-specific enum error type when you are following a well defined
specification (say HTTP error codes), and the enum is meant to be an ergonomic
way to represent the raw value dictated by the specification.

More information:
[FIDL API Rubric - Prefer domain specific enum for errors][fidl-error-enum].

### Don't use declarations from other libraries

It's good for a public API to reuse types and compose protocols if they're
semantically equivalent, but it's easy to make mistakes.

[calibration]: /docs/contribute/governance/api_council.md#calibration
[fidl-bool]: /docs/development/api/fidl.md#avoid_booleans_if_more_states_are_possible
[fidl-enum]: /docs/reference/fidl/language/language.md#strict-vs-flexible
[fidl-error-enum]: /docs/development/api/fidl.md#prefer-domain-specific-enum-for-errors
[fidl-error-syntax]: /docs/development/api/fidl.md#error-syntax
[fidl-flexible]: /docs/development/api/fidl.md#strict-flexible-method
[fidl-open]: /docs/development/api/fidl.md#open-ajar-closed
[fidl-table]: /docs/development/api/fidl.md#should-i-use-a-struct-or-a-table
[fidl-rubric]: /docs/development/api/fidl.md
[fidl-style]: /docs/development/languages/fidl/guides/style.md
[fidl-library-style]: /docs/development/languages/fidl/guides/style.md#library-overview
[fidl-string]: /docs/development/api/fidl.md#specify_bounds_for_vector_and_string
[fidl-versioning]: /docs/reference/fidl/language/versioning.md
[sdk-category]: /docs/contribute/sdk/categories.md
[sdk-category-promotion]: /docs/contribute/sdk/README.md#promoting-an-api
[version-json]: https://cs.opensource.google/fuchsia/fuchsia/+/main:/sdk/version_history.json
