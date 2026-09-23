# FIDL versioning

This document describes FIDL's API versioning features. For guidance on how to
evolve Fuchsia Platform APIs, see [Fuchsia API evolution guidelines][api-evolution].

## Summary

FIDL versioning lets you represent changes to a FIDL library over time. When
making a change, you use the `@available` attribute to describe when (i.e. at
which version) the change occurs. To generate bindings, you pass the
`--available` flag to fidlc specifying one or more versions.

FIDL versioning provides API versioning, not ABI versioning. There is no way to
query versions at runtime. Changes may be API breaking, but they are expected to
be ABI compatible. The FIDL compiler performs some basic validation, but it does
not guarantee ABI compatibility.

## Concepts

The unit of versioning is a group of libraries, called a **platform**. By
convention, libraries are named starting with the platform name. For example,
the libraries `fuchsia.mem` and `fuchsia.web` belong to the `fuchsia` platform.

Each platform has a linear **version** history. A version is an integer from 1
to 2^31-1 (inclusive), or one of the special versions `NEXT` and `HEAD`. The
`NEXT` version is used for changes that are planned to go in the next numbered
version. The `HEAD` version is used for the latest unstable changes.

If a FIDL library doesn't have any `@available` attributes, it belongs to the
`unversioned` platform. This platform only has one version, `HEAD`.
FIDL libraries in `fuchsia.git` must specify an `@available` attribute.

## Target versions {#target-versions}

When you target a single version, the bindings include all elements that are
available in that version as specified by `@available` arguments in the FIDL
files.

When you target a set of versions, the bindings include all elements that are
available in any of the versions in the set. For elements that are
[`replaced`](#replacing), the bindings only include the latest definition.

No matter what set of versions you target, if FIDL compilation succeeds, then it
is also guaranteed to succeed for all subsets of that set, and for all possible
singleton sets.

## Syntax

The `@available` attribute is allowed on any [FIDL element][element]. It takes
the following arguments:

| Argument     | Type      | Description                                                           |
| ------------ | --------- | --------------------------------------------------------------------- |
| `platform`   | `string`  | Library group name (see [Concepts](#concepts)); only allowed on `library`          |
| `added`      | `uint64`  | Integer, `NEXT`, or `HEAD`                                            |
| `deprecated` | `uint64`  | Integer, `NEXT`, or `HEAD`                                            |
| `removed`    | `uint64`  | Integer, `NEXT`, or `HEAD`                                            |
| `replaced`   | `uint64`  | Integer, `NEXT`, or `HEAD`                                            |
| `note`       | `string`  | Provides context for `deprecated`, `removed`, and/or `replaced`       |
| `renamed`    | `string`  | New name for `removed` or `replaced` element; only allowed on members |

There are some restrictions on the arguments:

- All arguments are optional, but at least one must be provided.
- Arguments must be literals, not references to `const` declarations.
- The `removed` and `replaced` arguments are mutually exclusive.
- Arguments must respect `added <= deprecated < removed`, or
  `added <= deprecated < replaced`.
- The `added`, `deprecated`, `removed`, and `replaced` arguments
  [inherit](#inheritance) from the parent element if unspecified.

For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="arguments" %}
```

If `@available` is used anywhere in a library, it must also appear on the
library declaration. For single-file libraries, this is straightforward. For
libraries with two or more `.fidl` files, only one file can have its library
declaration annotated. (The library is logically considered a single [element]
with attributes merged from each file, so annotating more than one file results
in a duplicate attribute error.) The FIDL style guide [recommends][overview]
creating a file named `overview.fidl` for this purpose.

On the library declaration, the `@available` attribute requires the `added`
argument and allows the `platform` argument. If the `platform` is omitted, it
defaults to the first component of the library name. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="library" %}
```

## Modifiers {#modifiers}

FIDL versioning lets you add or remove modifiers at specific versions. After a
modifier, you can write arguments in parentheses the same way you would after
`@available`. However, only the `added` and `removed` arguments are allowed.

Here is an example of changing an enum from strict to flexible:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="modifiers" %}
```

All modifiers support this syntax: `strict`, `flexible`, `resource`, `closed`,
`ajar`, and `open`. However, changing the `strict` or `flexible` modifier on a
two-way method without error syntax is [not allowed][fi-0219].

When you target a [set of versions](#target-versions), the compiler uses the
latest modifiers. In the example above, the enum would be flexible if the set
includes any version equal or greater to 2, even if it also includes 1.

## Inheritance {#inheritance}

The arguments to `@available` flow from the library declaration to top-level
declarations, and from each top-level declaration to its members. For example,
if a table is added at version 5, there is no need to repeat this annotation on
its members because they could not exist prior to the table itself. Here is a
more complicated example of inheritance:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="inheritance" %}
```

## Deprecation

Deprecation is used to indicate that an element will be removed in the future.
When you deprecate an element, you should add a `# Deprecation` section to the
doc comment with a detailed explanation, and a `note` argument to the
`@available` attribute with a brief instruction. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="deprecation" %}
```

As of June 2024 deprecation has no impact in bindings. However, the FIDL team
[plans][deprecation-bug] to make it emit deprecation annotations in target
languages. For instance, the example above could produce `#[deprecated = "Use
`Replacement`."]` in the Rust bindings.

## Identity {#identity}

FIDL versioning distinguishes between removing and [replacing](#replacing)
elements. To do this, it relies on a notion of API and ABI identity. The API
identity of an element is its name. ABI identity depends on the kind of element:

- Bits/enum member: value, e.g. 5 in `VALUE = 5;`
- Struct member: offset, e.g. 0 for first member
- Table/union member: ordinal, e.g. 5 in `5: name string;`
- Protocol member: selector, e.g. "example/Foo.Bar" in
  `library example; protocol Foo { Bar(); };`

Other elements, such as type declarations and protocols, have no ABI identity.

## Replacing {#replacing}

The `replaced` argument lets you change an element at a particular version by
writing a completely new definition. This is the only way to change certain
aspects of FIDL elements, including:

- The value of a constant
- The type of a struct, table, or union member
- The kind of a declaration, e.g. changing a struct to an alias
- The presence of `error` syntax on a method
- Attributes on the element

To replace an element at version `N`, annotate the old definition with
`@available(replaced=N)` and the new definition with `@available(added=N)`.
For example, here is how you change the value of a constant:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="replace_constant" %}
```

As another example, here is how you would change the type of a table field:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="replace_member" %}
```

The FIDL compiler verifies that for every `@available(replaced=N)` element there
is a matching `@available(added=N)` element with the same [identity](#identity).
It also verifies that every `@available(removed=N)` element **does not** have
such a replacement. This validation only applies to elements directly annotated,
not to elements that [inherit](#inheritance) the `removed` or `replaced`
argument.

## Renaming

To rename a member, [replace](#replacing) it with a new definition and specify
the new name with the `renamed` argument on the old definition. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="renamed" %}
```

The `renamed` argument is only allowed on members because the FIDL compiler
relies on their [ABI identity](#identity) to validate it. To rename a
declaration, simply remove the old definition in favor of a new one:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="rename_declaration" %}
```

### After removal {#after-removal}

Normally the `renamed` argument is used with `replaced=N`, but you can also use
it with `removed=N`. This gives a new name to refer to the member after its
removal. How it works is based on the set of [target
versions](#target-versions):

* If you only target versions less than `N`, the bindings will use the old name.
* If you only target versions equal to or greater than `N`, the bindings won't
  include the member at all.
* If you target a set containing versions less than `N` _and_ containing
  versions greater than or equal to `N`, the bindings will use the new name.

One reason to do this is to discourage new usage of an API while continuing to
support its implementation. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="discourage_use" %}
```

If the `Door` server is implemented in a codebase that targets the version set
{4, 5}, then the method will be named `DeprecatedOpen`, discouraging developers
from adding new uses of the method. If another codebase targets version 4 or
below, then the method will be named `Open`. If it targets version 5, the method
will not appear at all.

Another reason to use this feature is to reuse a name for a new ABI. For
example, consider changing the method `Open` to return an error:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="reuse_name" %}
```

We need to define a new method, since a client that doesn't expect an error will
close the channel if it receives an error response. However, we can keep using
the name `Open` as long as we (1) use `@selector` to give the new method a
different [ABI identity](#identity) and (2) use `renamed` on the old definition,
allowing bindings for the version set {4, 5} to include both methods.

## Referencing FIDL types {#references}

There are a variety of ways one FIDL element can reference another. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/versioning.test.fidl" region_tag="references" %}
```

When referencing elements, you must respect the `@available` attributes. For
example, the following code is invalid because `A` exists from version 1 onward,
but it tries to reference `B` which only exists at version 2:

```fidl
// Does not compile!

@available(added=1)
const A bool = B;

@available(added=2, removed=3)
const B bool = true;
```

Similarly, it is invalid for a non-deprecated element to reference a deprecated
element. For example, the following code is invalid at version 1 because `A`
references `B`, but `B` is deprecated while `A` is not.

```fidl
// Does not compile!

@available(deprecated=2)
const A bool = B;

@available(deprecated=1)
const B bool = true;
```

## `fidlc` command line

The FIDL compiler accepts the `--available` flag to specify platform versions.
For example, assuming `example.fidl` defines a library in the `fuchsia` platform
with no dependencies, you can compile it at version 22 as follows:

```posix-terminal
fidlc --available fuchsia:22 --out example.json --files example.fidl
```

You can target multiple versions by separating them with commas, e.g.
`--available fuchsia:19,22,23,NEXT,HEAD`.

If a library `A` has a dependency on a library `B` from a different platform,
you can specify versions for both platforms using the `--available` flag twice.
However, `A` must be compatible across its entire version history with the fixed
version chosen for `B`.


[element]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md#terminology
[overview]: /docs/development/languages/fidl/guides/style.md#library-overview
[deprecation-bug]: https://bugs.fuchsia.dev/p/fuchsia/issues/detail?id=7692
[api-evolution]: /docs/development/api/evolution.md
[fi-0219]: /docs/reference/fidl/language/errcat.md#fi-0219
