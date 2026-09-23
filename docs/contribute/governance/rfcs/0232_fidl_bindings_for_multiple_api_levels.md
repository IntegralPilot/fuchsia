<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0232" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

Note: This RFC is an amendment to [RFC-0083: FIDL versioning][rfc-0083], and it
is dependent on [RFC-0231: FIDL versioning replacement syntax][rfc-0231].

## Summary

When generating FIDL bindings today, we target [`LEGACY`][legacy] in tree and a
numbered API level out of tree. This document proposes to generalize and
obsolete `LEGACY` by providing a way to target multiple API levels at once. This
is only intended to be used in tree initially, but we may find situations where
will be useful out of tree.

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

There are a few problems with `LEGACY`:

* It's a pseudo-version that carries no information on its own. Bindings for a
  frozen API level `N` contain all APIs that are part of `N`, but bindings for
  `LEGACY` contain whatever happened to be marked `removed` with `legacy=true`
  at the time of the build (in addition to everything in `HEAD`).

* Legacy support is determined on a per-API basis, and changes over time. This
  makes it difficult to guarantee that a platform build actually supports a
  given older API level.

* It is not possible to target a specific API level (i.e. other than `HEAD`)
  while including legacy support.

* It only solves a subset of compatibility challenges, where the Fuchsia
  platform is one side of the communication. There are protocols that are spoken
  between product components, where this is not the case.

* It privileges the Fuchsia monorepo so that it becomes harder to split
  components out of the monolithic repository and release process.

## Stakeholders

_Facilitator:_ abarth@google.com

_Reviewers:_ hjfreyer@google.com, wilkinsonclay@google.com, ddorwin@google.com

_Consulted:_ wez@google.com, sethladd@google.com

_Socialization:_ I discussed this idea with the FIDL team and the Platform
Versioning working group before writing the RFC.

## Design

We propose to allow targeting multiple API levels at once when generating FIDL
bindings. For example, invoking fidlc with `--available fuchsia:10,15` would
target API levels 10 and 15, resulting in bindings that combine elements from
both levels. If an element with a given name has different definitions under 10
and 15, we use the definition from level 15 because it is newer.

This obsoletes the `LEGACY` version. When building the Fuchsia platform, instead
of targeting `LEGACY` bindings, we will target the set of supported API levels.
This also eliminates the need to mark individual APIs with `legacy=true`.

### Details

* Remove the `LEGACY` version, and the `@available` attribute's `legacy`
  argument, from the FIDL language.

* Change fidlc's `--available` command line argument syntax from
  `<platform>:<target_version>` to `<platform>:<target_versions>` where
  `<target_versions>` is a comma-separated list of versions. Examples:

    * `--available fuchsia:10`
    * `--available fuchsia:10,11`
    * `--available fuchsia:10,20,HEAD`

* The `<target_versions>` list must be sorted and must not contain duplicates.
  This is to emphasize the fact that versions create a linear history, and later
  versions are treated preferentially.

* The `<target_version>` list determines a set of _candidate elements_:

    * An element marked `@available(added=A)` is a candidate element if
      `<target_versions>` intersects `{v | v >= A}`.

    * An element marked `@available(added=A, removed=R)` is a candidate element
      if `<target_versions>` intersects `{v | A <= v < R}`.

    * Note that this RFC is dependent on [RFC-0231: FIDL versioning replacement
      syntax][rfc-0231]. For the purposes of determining candidates elements,
      `replaced` is treated the same as `removed`.

* An element is included in bindings if (1) it is a candidate element, and (2)
  among all candidates with the same name, its `added` version is greatest.

* If an element marked `@available(..., deprecated=D, ...)` is included in
  bindings by the rules above, it is considered deprecated in bindings if
  `<target_versions>` intersects `{v | v >= D}`. This has no impact on generated
  code today, but may in the future (https://fxbug.dev/42156877).

* As before, the `--available` flag can be used more than once for multiple
  platforms. There is no significant interaction between these two features
  (multiple platforms and multiple target versions).

* As before, the success or failure of compilation **MUST** be independent of
  the `--available` flag for the main library's platform. (It may be dependent
  on the `--available` flag for a dependency in another platform.) For example,
  if compilation succeeds with `--available fuchsia:15,16`, it is guaranteed to
  also succeed with `--available fuchsia:10,100,HEAD`. Similarly, if the former
  fails, the latter is guaranteed to fail with the same set of errors.

* When building the Fuchsia platform, replace `--available fuchsia:LEGACY` with
  `--available fuchsia:<target_versions>` where `<target_versions>` includes all
  runtime supported API levels, the in-development API level, and `HEAD`.

### Implications

This design allows generating bindings for a valid FIDL library that target any
arbitrary set of versions, regardless of how the library has evolved over time.
This is a significant constraint, since FIDL versioning can represent any
syntactically valid change. In particular, fidlc allows multiple elements with
the same name to coexist as long as their version ranges do not overlap. When
`<target_versions>` would include multiple such elements, we only include the
newest element. This supports three general patterns of evolution:

- _Lifecycle._ An element is `added` and possibly `removed`. We include it in
  bindings when targeting any version within its lifecycle. Example:

  ```fidl
  @available(added=1, removed=5)
  flexible Method() -> ();
  ```

- _Replacement._ An element is `added`, and later [`replaced`][rfc-0231] with a
  different definition. Conceptually, this represents a single element changing
  over time, not two distinct elements. We assume the replacement was designed
  to be compatible with the original element, and only include the replacement
  element in bindings. Example:

  ```fidl
  @available(added=1, replaced=5)
  flexible Method() -> ();
  @available(added=5)
  flexible Method() -> () error uint32;
  ```

- _Name reuse_. After an element is `removed`, its name can be reused for a new
  element `added` later. This is like _replacement_, but the two elements are
  conceptually distinct, and there is a gap between their lifecycles. We assume
  the newer element is preferred, and only include it in bindings. Example:

  ```fidl
  @available(added=1, removed=5)
  flexible Method();
  @available(added=10)
  flexible Method() -> ();
  ```

  Note that when an element name is reused in this way, references to it cannot
  span the gap between the two definitions. For example, this would not compile:

  ```fidl
  @available(added=1, removed=5)
  type Args = struct {};
  @available(added=10)
  type Args = table {};

  @available(added=2)
  protocol Foo {
      Method(Args); // ERROR: 'Method' exists at versions 5 to 10, but 'Args' does not
  };
  ```

### Examples

Consider the following FIDL library:

```fidl
@available(added=1)
library foo;

@available(replaced=2)
type E = strict enum { V = 1; }; // E1
@available(added=2)
type E = flexible enum { V = 1; }; // E2

@available(added=3, removed=6)
open protocol P {
    @available(removed=4)
    flexible M() -> (); // M1

    @available(added=5)
    flexible M(table {}) -> (); // M2
};
```

Here is what's included in bindings when selecting a single version:

| `--available` | E1 | E2 | P | M1 | M2 |
|---------------|----|----|---|----|----|
| `foo:1`       | ✔︎  |    |   |    |    |
| `foo:2`       |    | ✔︎  |   |    |    |
| `foo:3`       |    | ✔︎  | ✔︎ | ✔︎  |    |
| `foo:4`       |    | ✔︎  | ✔︎ |    |    |
| `foo:5`       |    | ✔︎  | ✔︎ |    | ✔︎  |
| `foo:6`       |    | ✔︎  |   |    |    |
| `foo:HEAD`    |    | ✔︎  |   |    |    |

And here is what's included when selecting multiple versions:

| `--available`             | E1 | E2 | P | M1 | M2 |
|---------------------------|----|----|---|----|----|
| `foo:1,2`                 |    | ✔︎  |   |    |    |
| `foo:1,HEAD`              |    | ✔︎  |   |    |    |
| `foo:1,3`                 |    | ✔︎  | ✔︎ | ✔︎  |    |
| `foo:1,2,3`               |    | ✔︎  | ✔︎ | ✔︎  |    |
| `foo:3,6`                 |    | ✔︎  | ✔︎ | ✔︎  |    |
| `foo:3,HEAD`              |    | ✔︎  | ✔︎ | ✔︎  |    |
| `foo:2,4,6`               |    | ✔︎  | ✔︎ |    |    |
| `foo:1,3,5`               |    | ✔︎  | ✔︎ |    | ✔︎  |
| `foo:1,2,3,4,5,6,HEAD`    |    | ✔︎  | ✔︎ |    | ✔︎  |

## Implementation

1. Implement [RFC-0231: FIDL versioning replacement syntax][rfc-0231].

2. Implement the new `--available` functionality in fidlc. Also change the
   "available" property in the JSON IR to use an array of strings for versions.

3. Change all existing `legacy` arguments to line up with the new system (i.e.
   `false` if removed before our minimum supported API level, and `true` if
   removed on or after it). If there is a large discrepancy, consider
   [Alternative: Override mechanism](#alt-override).

4. Change the in-tree platform build to generate bindings targeting all
   supported API levels, the in-development API level, and `HEAD`.

5. Remove all `legacy` arguments in FIDL files.

6. Remove `LEGACY` support from fidlc.

7. Retire fidlc error codes [fi-0182] and [fi-0183].

## Performance

This proposal has no impact on performance.

## Ergonomics

This proposal makes FIDL versioning easier to use correctly, since there is no
need to worry about the `legacy` argument anymore.

## Backwards compatibility

This proposal helps to achieve ABI backward compatibility, since it removes the
burden of choosing `legacy=true` from individual FIDL library authors. It also
gives more credence to our stated set of "supported API levels", since those API
levels are directly used to generate bindings for the platform. (Of course, to
truly be confident they are supported we need tests as well.)

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

### Non-issue: Reduced incentive to migrate

This proposal could be seen as reducing the incentive [described in RFC-0002:
Platform Versioning][rfc-0002-dynamics] to migrate off deprecated APIs, since
you can access both new and old APIs by targeting multiple levels. However, this
is already possible today with `LEGACY`. Just as petals should not target
`LEGACY` today, they should not misuse this new feature.

Also, since petals use fidlc via the SDK rather than invoking it directly, we
can mitigate this with restrictions in the SDK build rules. For example, they
could assert that the target version string does not contain a comma.

### Alternative: Version ranges

Instead of allowing an arbitrary set of versions, we could require a range
specified by two endpoints. I rejected this alternative for a few reasons:

* If we decide to increase the cadence of API levels, it might be easier to only
  maintain long-term support for some fraction of them. This would lead to a set
  of versions with gaps, not a range.

* We might want to support an individual old component that targets API level
  `N` without recopmiling it. If everything else has already moved off API
  levels `N` through `M`, we could have a gap of `{N+1, ..., M}`.

* Nothing we've built for platform versioning so far has assumed a contiguous
  range of supported API levels. For example, [version_history.json] contains a
  list of API levels, not a range.

* Using ranges instead of sets would _not_ make the fidlc implementation easier.
  It might make it slightly more efficient, but this is unlikely to matter in
  practice. There are many lower hanging fruit to optimize should fidlc
  performance ever become a problem.

### Alternative: Override mechanism {#alt-override}

One drawback of this proposal is that it could be difficult to update all code
in fuchsia.git atomically when dropping support for an API level. To split such
changes into multiple steps, we might want a more granular way to control what
gets included in bindings. There are a few options for that:

1. Override `<target_versions>` in individual `fidl` GN targets.

2. Add an `@available` argument `unsupported=true` which excludes the element
   even if it would normally be included. This is similar to `legacy`, but would
   only be used temporarily (ideally).

3. Change the `--available` argument to accept a JSON file which, in addition to
   `<target_versions>`, can give a list of fully qualified element names to
   include or exclude.

I rejected this alternative because it's not clear we will need this mechanism.
Instead, we should first try making changes in a single CL. If that doesn't
work, we should try using conditional compilation to stage changes, so that
implementations are only included before dropping support for the API level. If
that doesn't work, we can revisit the override mechanisms above.

We could also mitigate this by increasing the cadence of API levels, which would
result in fewer removals per API level. However, this would have many other
implications for platform versioning and is out of scope in this proposal.

### Alternative: Make `legacy` true by default

See [RFC-0233: FIDL legacy by default][rfc-0233].

This alternative improves on the status quo. With `false` as the default,
forgetting to add `legacy=true` can cause ABI breakage. With `true` as the
default, forgetting to add `legacy=false` can only result in a fidlc compile
error or unused APIs in bindings, a much less severe problem.

However, this is only a minor change and does not address all the problems
raised in this RFC. The `legacy` state would still be controlled per API,
leading to inconsistent runtime support for a given API level, and making it
hard to determine whether a particular build fully supports an API level.

## Prior art and references

The Android SDK allows specifying `compileSdkVersion` and `minSdkVersion`. See
[Android API Levels] and the [`<uses-sdk>` documentation][uses-sdk].

[Android API Levels]: https://apilevels.com/#definitions
[attributes]: /docs/reference/fidl/language/attributes.md
[evolution]: /docs/development/api/evolution.md
[fi-0182]: https://fuchsia.dev/error/fi-0182
[fi-0183]: https://fuchsia.dev/error/fi-0183
[legacy]: /docs/reference/fidl/language/versioning.md#legacy
[rfc-0002-dynamics]: /docs/contribute/governance/rfcs/0002_platform_versioning.md#dynamics
[rfc-0083-amendment]: https://fuchsia-review.googlesource.com/c/fuchsia/+/734932
[rfc-0083]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md
[rfc-0231]: /docs/contribute/governance/rfcs/0231_fidl_versioning_replacement_syntax.md
[rfc-0233]: /docs/contribute/governance/rfcs/0233_fidl_legacy_by_default.md
[swapping]: https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/reference/fidl/language/versioning.md;l=189;drc=818df65453cad51dd8351cd88074d606a00575e8
[uses-sdk]: https://developer.android.com/guide/topics/manifest/uses-sdk-element
[version_history.json]: /sdk/version_history.json
[versioning]: /docs/reference/fidl/language/versioning.md
