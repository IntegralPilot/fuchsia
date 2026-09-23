<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0231" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

Note: This RFC is an amendment to [RFC-0083: FIDL versioning][rfc-0083].

## Summary

Change FIDL's `@available` attribute to support a `replaced` argument. It is
like `removed`, but validates that a replacement is `added` at the same version.

## Background {#background}

[RFC-0083: FIDL versioning][rfc-0083] introduced the `@available` attribute for
versioning FIDL APIs. One requirement in that design was to allow versioning all
possible changes. In other words, it should be possible to express `v1.fidl`
transitioning to an arbitrarily different `v2.fidl` in a single `versioned.fidl`
file. The design satisfied that requirement by allowing multiple elements to
have the same name as long as their version ranges don't overlap.

For example, consider adding the `@discoverable` attribute to protocol `P` at
version 5. We can't express this directly with `@available`, since attributes
can't be placed on other attributes. However, we can instead do this:

```fidl
@available(added=1, removed=5)
protocol P {};

@available(added=5)
@discoverable
protocol P {};
```

This technique of removing an element and re-adding it at the same version is
known as [swapping].

## Motivation

The syntax for the usual lifecycle of adding, deprecating, and removing APIs is
intuitive. The swapping pattern, on the other hand, is not. It makes sense when
explained, but it is a hidden feature that requires explanation. In the [example
above](#background), you'd be forgiven for thinking `removed=5` really means the
protocol is gone in version 5, especially if the corresponding `added=5` is not
adjacent in the source code.

Distinguishing replacement from removal also makes it possible to get rid of the
[legacy] feature and replace it with a more general solution. This is a separate
proposal: [RFC-0232: FIDL bindings for multiple API levels][rfc-0232].

## Stakeholders

_Facilitator:_ abarth@google.com

_Reviewers:_ hjfreyer@google.com, ianloic@google.com, ddorwin@google.com

_Consulted:_ wez@google.com, sethladd@google.com, wilkinsonclay@google.com

_Socialization:_ I discussed this idea with the FIDL team and the Platform
Versioning working group before writing the RFC.

## Design

* Introduce a new `@available` argument named `replaced`. It behaves the same as
  `removed` except it has different validation, described below. It can be
  used instead of `removed` on any element except the `library` declaration.

* When an element is marked `removed=N`, validate that there **IS NOT** another
  element with the same name marked `added=N`.

* When an element is marked `replaced=N`, validate that there **IS** another
  element with the same name marked `added=N`.

* This validation only applies to elements that are directly marked `removed` or
  `replaced`, not to child elements that [inherit] the arguments.

### Examples

The example from the [Background section](#background) of adding `@discoverable`
to a protocol at version 5 looks like this in the new design:

```fidl
@available(added=1, replaced=5)
protocol P {};

@available(added=5)
@discoverable
protocol P {};
```

As another example, consider replacing a struct that contains a member with an
anonymous type:

```fidl
@available(added=1, replaced=2)
struct Foo {
    bar @generated_name("Bar") table {};
};

@available(added=2)
struct Foo {
    baz string;
};
```

Since the first `Foo` is marked `replaced=2`, fidlc validates that there is
another `Foo` marked `added=2`. However, it does _not_ perform similar
validation for the child element `bar`, nor for its anonymous type `Bar`.

## Implementation

1. Implement the `replaced` argument, including its validation.

2. Change all FIDL files to use `replaced` instead of `removed` when applicable.

3. Implement the new `removed` validation. CQ will fail if (2) missed anything.

## Performance

This proposal has no impact on performance.

## Ergonomics

This proposal makes the [swapping] pattern more ergonomic by directly supporting
it in the language.

## Backwards compatibility

This proposal has no impact on backwards compatibility.

## Security considerations

This proposal has no impact on security.

## Privacy considerations

This proposal has no impact on privacy.

## Testing

This following files must be updated to test the new behavior:

* tools/fidl/fidlc/tests/availability_interleaving_tests.cc
* tools/fidl/fidlc/tests/decomposition_tests.cc
* tools/fidl/fidlc/tests/versioning_tests.cc
* tools/fidl/fidlc/tests/versioning_types_tests.cc

## Documentation

The following pages must be updated to document `replaced`:

* [FIDL attributes][attributes]
* [FIDL versioning][versioning]
* [Fuchsia API evolution guidelines][evolution]

The term "swapping" should also be removed in favor of "replaced".

The [fidldoc] tool currently operates on the JSON IR for a single API level, so
it never sees `removed` arguments, and will not see `replaced` either
(https://fxbug.dev/42084086). When this is fixed, fidldoc should be updated to
clearly indicate when something is replaced as opposed to removed.

## Drawbacks, alternatives, and unknowns

### Alternative: Re-added argument

This proposal introduces the `replaced` argument, which makes it clear when an
element is being replaced rather than removed. It prevents the scenario, "I
thought I couldn't use this API because it says `removed=5` and we target 6."

We could similarly introduce a `re_added` argument to clarify when an element is
added as a replacement rather than being added for the first time. It would
prevent the scenario, "I thought I couldn't use this API yet because it says
`added=6` and we target 5."

I rejected this alternative for a few reasons:

* I believe the first scenario is more important than the second.

* The name `re_added` is unsatisfactory, and I couldn't think of a better one.

* Even without this, we can still make [fidldoc] infer whether an element is
  `added` for the first time or not.

## Prior art and references

I discussed a similar idea in 2021 while developing FIDL versioning, in an
internal document titled "FIDL versioning: Swapping elements".

This proposal is very specific to the design of FIDL versioning, so there is no
prior art on this exact problem, as far as I know.

[attributes]: /docs/reference/fidl/language/attributes.md
[evolution]: /docs/development/api/evolution.md
[fidldoc]: /tools/fidl/fidldoc/README.md
[inherit]: /docs/reference/fidl/language/versioning.md#inheritance
[legacy]: /docs/reference/fidl/language/versioning.md#legacy
[rfc-0083]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md
[rfc-0232]: /docs/contribute/governance/rfcs/0232_fidl_bindings_for_multiple_api_levels.md
[swapping]: https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/reference/fidl/language/versioning.md;l=189;drc=818df65453cad51dd8351cd88074d606a00575e8
[versioning]: /docs/reference/fidl/language/versioning.md
