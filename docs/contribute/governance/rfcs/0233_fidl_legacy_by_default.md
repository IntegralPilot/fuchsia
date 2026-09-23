<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0233" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

Note: This RFC is an amendment to [RFC-0083: FIDL versioning][rfc-0083].

## Rejection Rationale

This RFC was rejected in favor of [RFC-0232: FIDL bindings for multiple API
levels][rfc-0232]. The goal of both proposals was to address the shortcomings of
the `legacy` feature. This one offered an incremental improvement, while
RFC-0232 got rid of the feature entirely, replacing it with something better.
With `legacy` gone, this RFC is no longer relevant.

## Summary

Change the FIDL `@available` attribute to make `legacy=true` the default, so
that `legacy=false` is required to opt out of `LEGACY` inclusion.

## Background

With the original design of FIDL versioning, it was impossible to remove an
element while retaining ABI support for it. For example, if a CL marked a method
as `removed=5`, it would also have to delete the implementation of that method.
That is because we built the Fuchsia platform at `HEAD`, and the method's server
bindings would no longer exist at `HEAD` since it is greater than 5.

To solve this, we [amended][rfc-0083-amendment] RFC-0083 to introduce the
`LEGACY` version and the `legacy` argument. The `LEGACY` version is like `HEAD`,
except it re-adds removed elements if they are marked `legacy=true`.

## Motivation

People are confused about whether they should write `legacy=true` when removing
a FIDL API. The answer is: almost always, unless you are [swapping]. Moreover,
the consequence of incorrectly using `legacy=false` (ABI breakage) is much worse
than that of `legacy=true` (a fidlc compile error, or unused APIs in bindings)
All this suggests the default should be flipped.

## Stakeholders

_Facilitator:_ abarth@google.com

_Reviewers:_ hjfreyer@google.com, ianloic@google.com

_Consulted:_ wez@google.com, sethladd@google.com, wilkinsonclay@google.com

_Socialization:_ I discussed this idea with the FIDL team and the Platform
Versioning working group before writing the RFC.

## Design

Change the `@available` attribute's `legacy` argument to be true by default for
`removed` elements. Allow `legacy=false` to override the default.

## Implementation

1. Change all FIDL files to explicitly specify `legacy=false` on all `removed`
   elements that do not have `legacy=true`.

2. Temporarily make `legacy` a required argument for `removed` elements.
   CQ will fail here if (1) missed anything.

3. Make `legacy` optional again, and true by default.

4. Change all FIDL files to remove occurrences of `legacy=true`.

## Performance

This proposal has no impact on performance.

## Ergonomics

This proposal makes FIDL versioning easier to use correctly. In particular, it
removes the pitfall of forgetting to write `legacy=true` when removing an
element.

## Backwards compatibility

This proposal helps to achieve ABI backward compatibility, since it makes the
syntax for ABI compatibility opt-out rather than opt-in.

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

The following documentation pages must be updated:

* [FIDL attributes][attributes]
* [FIDL versioning][versioning]
* [Fuchsia API evolution guidelines][evolution]

## Drawbacks, alternatives, and unknowns

### Alternative: Different default for `removed` and `replaced` {#alt-removed-replaced}

Under this proposal, [swapping] elements will require writing `legacy=false`.
For example, to change an enum from strict to flexible at version 10:

```
@available(removed=10, legacy=false)
type Foo = strict enum { VAL = 1; };

@available(added=10)
type Foo = flexible enum { VAL = 1; };
```

If [RFC-0231: FIDL versioning replacement syntax][rfc-0231] is also accepted, we
could make the default true for `removed` and false for `replaced`.

### Alternative: Remove `legacy` argument

With the new default, why keep the `legacy` argument at all? There are three use
cases for removing an element with `legacy=false`:

1. [Swapping]. The [alternative above](#alt-removed-replaced) addresses this.

2. Removing a member of a flexible data structure, in some cases. Consider
   removing a table field. If the legacy-supporting component would simply
   ignore the field, then it doesn't need bindings for it in `LEGACY`.

3. Dropping support for an old API level and removing the implementation.

Use case (2) is probably rare, and not essential. There's not much harm in
having unused `LEGACY` bindings for such members.

Use case (3) is real, but instead of `legacy=false`, we could simply delete code
from FIDL files. This would mean we could no longer regenerate documentation for
old API levels, but we could store pre-generated documentation instead.

### Alternative: Replace `LEGACY` with target/minimum levels

See https://fxrev.dev/ **TODO**

## Prior art and references

I did not look into prior art on this because it is simply a proposal to change
a default value.

[attributes]: /docs/reference/fidl/language/attributes.md
[evolution]: /docs/development/api/evolution.md
[rfc-0083-amendment]: https://fuchsia-review.googlesource.com/c/fuchsia/+/734932
[rfc-0083]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md
[rfc-0231]: /docs/contribute/governance/rfcs/0231_fidl_versioning_replacement_syntax.md
[rfc-0232]: /docs/contribute/governance/rfcs/0232_fidl_bindings_for_multiple_api_levels.md
[swapping]: https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/reference/fidl/language/versioning.md;l=189;drc=818df65453cad51dd8351cd88074d606a00575e8
[versioning]: /docs/reference/fidl/language/versioning.md
