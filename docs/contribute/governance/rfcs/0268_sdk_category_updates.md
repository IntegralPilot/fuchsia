<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0268" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Update the set of [SDK categories documented in RFC-0165] to reflect current
usage and clarify the meanings.

## Summary

This RFC changes the set of SDK categories and contains updated language
describing the meaning and use of all SDK categories. These categories replace
the [SDK categories documented in RFC-0165]. The category details in the
[Design](#design) section are written such that they could be used to update the
[SDK categories documentation].

## Stakeholders

_Facilitator:_ neelsa@google.com

_Reviewers:_

- Platform Versioning: hjfreyer@google.com
- Developer: wilkinsonclay@google.com
- Testing: crjohns@google.com

_Socialization:_

The changes in this RFC were proposed and discussed with the Platform Versioning
team. The requirements for CTF tests were discussed in [issue 367760026] and
related issues.

## Requirements

The goals of updating the categories are:

- Reduce confusion about the SDK categories and when to use them. This includes
  removing unused and obsolete categories.
- Specifically, address confusion about `partner_internal`, which is often
  misunderstood, in part due to its ambiguous name and the inclusion of
  "internal."
    - This category was not one of the [SDK categories documented in RFC-0165];
      it was the outcome of an [API Council thread].
    - This category is used for ABIs used by both a) precompiled libraries
       shipped in the SDK and b) host tools. Although added for the former,
       there are twice as many uses for the latter. In addition, the
       [RealmFactory SDK requirements] currently suggest using this category.
    - Libraries used in shipping components and tools used by developers may
      have very different compatibility window requirements - a difference that
      could grow significantly as Fuchsia matures and the number of API levels
      in the [Sunset] phase increases.
- Define a solution for stabilizing APIs used by CTF tests without the overhead
  or commitment of shipping an API in the SDK ([issue 367760026]).
- Make it possible to have processes and compatibility windows that differ
  based on the use case and exposure or risk.
    - Specifically, reduce the overhead of stabilizing APIs needed for CTF tests
      and used by host tools, which in the past used unstable APIs and thus
      risked compatibility breakages.

This RFC does not attempt to address theoretical or very uncommon scenarios,
such as those described in [issue 369892217].


## Design

As in [RFC-0165], the new categories are in order of monotonically increasing
audience. Existing categories that are replaced are indicated in parentheses.

- `compat_test` (formerly `cts`)
- `host_tool` (formerly within `partner_internal`)
- `prebuilt` (formerly within `partner_internal`)
- `partner`

Each category name is a singular noun describing its audience - or where APIs in
that category can be used. To name the audience, add "developers" to the
category name. For example, "host tool developers."

In addition to the categories being replaced above, the following
[SDK categories documented in RFC-0165] are removed:

- `excluded`: Functionally equivalent to not specifying an SDK category. This
  category was rarely used, and it was not clear whether those uses indicated
  the atoms should _never_ be included in an SDK. Comments, which are more
  flexible and less ambiguous, can be used instead.
- `experimental`: Described as not making much sense in [RFC-0165].
- `internal`: Equivalent to not specifying an SDK category. (Most uses have been
   removed. The remaining uses have repurposed this category - see [issue
   372986936]. Limited support for this category will remain while those use
   cases are burned down.)
- `public`: Never implemented. We can re-evaluate the mechanism when broadening
  the audience for the SDK. Such a category can easily be added in the future.

With the removal of `public`, `partner` is equivalent to "published in the SDK."
The existing name is retained due to its extensive use, the broad awareness and
understanding of its meaning, and to potentially accommodate `public` or similar
in the future.

The following sections go into detail on the meaning and compatibility
requirements for each category, beginning with a short description that can be
used in places like the `sdk_atom()` template. The following summarizes the key
points:

- Libraries in the `prebuilt` and `partner` categories may be used in
  production use cases on end user devices and therefore have the same
  compatibility requirements for the platform implementation.
- Only APIs in the `partner` category may be used directly by out-of-tree
  developers (SDK consumers).
- All categories may be used for FIDL libraries. Other SDK atom types are
  generally exposed directly to developers, so the other categories are unlikely
  to make sense for them.
    - For this reason, the current implementation only allows non-FIDL atom
      types to be in the `partner` category.
- Unstable libraries (those exposed in an SDK but only at [`HEAD`][HEAD] and
  without compatibility guarantees) are allowed in  the `partner` category. The
  purpose of the other categories is compatibility, so unstable libraries are
  unlikely to make sense for them.
    - For this reason, the current implementation only allows libraries in the
      `partner` category to be unstable.
- Libraries in all categories are treated the same with respect to mechanisms
  intended to detect and prevent changes to stable API levels.
    - Currently, the only such tests are comparisons to API summary golden files
      for FIDL libraries. Each stable numerical API level is snapshotted upon
      creation. Snapshots for `NEXT` are also maintained as a mechainsm for
      API change detection and approval. See [SDK history] for more information.

The table below summarizes the information above. Columns reflecting the current
implementation are indicated as such.

<table>
  <tr>
   <td><strong>Name</strong>
   </td>
   <td><strong>Previous category</strong>
   </td>
   <td><strong>May be used in production on end user devices</strong>
   </td>
   <td><strong>Directly usable by OOT devs ("in the SDK")</strong>
   </td>
   <td><strong>Change detection enforced</strong>
   </td>
   <td><strong>Use with FIDL libraries?</strong>
   </td>
   <td><strong>Use with other SDK atom types? (current implementation)</strong>
   </td>
   <td><strong>May be unstable (current implementation)</strong>
   </td>
  </tr>
  <tr>
   <td><strong><code>compat_test</code></strong>
   </td>
   <td><code>cts</code> (not implemented)
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #fff2cc">No
   </td>
  </tr>
  <tr>
   <td><strong><code>host_tool</code></strong>
   </td>
   <td>Within <code>partner_internal</code>
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #fff2cc">No
   </td>
  </tr>
  <tr>
   <td><strong><code>prebuilt</code></strong>
   </td>
   <td>Within <code>partner_internal</code>
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #fff2cc">No
   </td>
   <td style="background-color: #fff2cc">No
   </td>
  </tr>
  <tr>
   <td><strong><code>partner</code></strong>
   </td>
   <td>N/A
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
   <td style="background-color: #d9ead3">Yes
   </td>
  </tr>
</table>

### compat_test

_"May be used to configure and run [compatibility tests] (e.g., CTF) tests but
may not be exposed for use in production in the SDK or used by host tools."_

NOTE: This section uses "CTF" in place of "compatibility tests" for clarity in
the context of current mechanisms and practice. However, this RFC does not
prohibit use in similar types of platform compatibility tests that may emerge in
the future.

Libraries in this category most likely contain a CTF [test realm factory]
protocol or are made available to tests via the
`fuchsia.testing.harness/RealmProxy` or `fuchsia.component.sandbox/Dictionary`
returned by a realm factory. These protocols provide a stable testing harness
for comparing the behavior of the same operations between versions of Fuchsia,
and the harness protocols themselves must also have compatibility guarantees to
ensure the compatibility tests can be run on the relevant Fuchsia versions.

While APIs in this category may be used in production code within the platform,
they may only be exposed outside the platform via CTF test realms or similar.

NOTE: FIDL libraries in this category, as in all SDK categories, must be
versioned in the `"fuchsia"` [FIDL platform]. Thus, if the library name begins
with `test.` or any string other than `fuchsia.`, `platform="fuchsia"` must be
specified in the library's `@available` attribute.

- **Exposure:** CTF tests and the Fuchsia platform developers responsible for
  them. The CTF tests are important because they help ensure ABI compatibility,
  especially of runtime support for previous API levels.
- **ABI compatibility window:** APIs must be supported as long as we need to run
   the relevant CTF tests.
    - In general, this means as long as tests for API levels in the [Supported]
      or [Sunset] phases ("run time support") use them.
    - We _could_ choose to stop running the tests, rebuild the tests to use a
      different API, or introduce a proxy to use a new API.

### host_tool

_"May be used by host tools (e.g., ffx) provided by the platform organization
but may not be used by production code or prebuilt binaries in the SDK."_

- **Exposure:** Developers using the platform-provided tools to interact with a
  target Fuchsia device supporting API level(s) supported by the tools.
- **ABI compatibility window:** APIs must be supported until the platform no
  longer supports communicating with host tools at any of the API levels at
  which the APIs are supported.
    - This is related to using host tools from _different_ releases to
      communicate with a given platform release.
    - NOTE: The set of API levels for which the platform provides this host tool
      runtime support could differ from either or both of:
        - The set of component target API levels that the current platform
          release supports (component runtime compatibility).
        - The set of API levels that host tools in the current release support
          using to communicate with a target device.
- Because APIs in this category are not used in an IDK/SDK:
    - They cannot (without hacking) be used by any non-platform components
      (i.e., those built by out-of-tree developers).
    - We can break them without impacting end users, as long as we are willing
      to deal with the implications for downstream developers. We would also
      need to consider any uses by CTF tests.

Implementation note: Host tools in the IDK/SDK have the category `"partner"` but
may use APIs from this category. However, APIs, code, precompiled libraries, and
prebuilt packages in `"partner"` may not use APIs in this category.

### prebuilt

_"May be part of the ABI that prebuilt binaries included in the SDK use to
interact with the platform. APIs in this category are not available in the SDK
for direct use by out-of-tree developers."_

Prebuilt binaries in the SDK (for use in production) currently include
precompiled static libraries, precompiled shared libraries, and packages. From
an out-of-tree developer's point of view, these are part of the implementation
details of the binary and are not used in the developer-facing API for the
library or packaged components. Source sets and libraries used internally by
prebuilt libraries and packages do not need to be in any SDK category, but the
APIs they use to interact with the platform do.

- **Exposure:** End users.
- **ABI compatibility window:** APIs must be supported as long as any API levels
  that support them are in the [Supported] or [Sunset] phase. That is, the same
  window as `partner`.
- Because the platform team controls all software using APIs in this category:
    - We do not necessarily need to worry as much about developer ergonomics.
    - We do not need to worry about use by third-party components.
    - We can replace uses in SDK precompiled libraries and prebuilt packages for
      new API levels ([`NEXT`][NEXT]) and thus drop runtime support (without
      deprecation) much faster than if we were concerned about downstream uses.

### partner

_"Included in the SDK for direct use of the API by out-of-tree developers."_

- **Exposure:** End users, out-of-tree developers, and [product owners].
- **ABI compatibility window:** APIs must be supported as long as any API levels
  that support them are in the [Supported] or [Sunset] phase.
- This category includes capabilities routed to or from components by
  out-of-tree developers and/or product components.
    - For example, capabilities product owners must route to the platform and
      capabilities that must be routed to or from prebuilt packages in the SDK.

## Implementation

The implementation will be done in the following stages:

1. `excluded` and `experimental` have already been removed.
2. Remove references to `public`.
3. Replace `cts` with `compat_test` and implement it.
4. Add FIDL libraries tracked in https://fxbug.dev/365602422 to the
   `compat_test` category and remove them from the allowlist.
5. Replace `partner_internal` with `host_test` and `prebuilt`.
    - Update the SDK marker mechanism used by ffx plugins and tools to allow use
      of both.
    - Add accommodations to enable deferring reassignment of FIDL libraries to
      the next step.
6. Assign FIDL libraries using `partner_internal` to `host_test` or
   `prebuilt` as appropriate.
7. Eventually, remove support for `internal` once the existing uses have been
   promoted to a supported category or removed ([issue 372986936]).

## Performance

There is no performance impact as the categories only affect the build and only
the strings used within existing mechanisms are changed.

## Ergonomics

The mechanism used by platform developers remains the same. The smaller number
of categories and more precise names should make it easier to understand what
they mean and which one is appropriate for a given use case.

## Backwards Compatibility

This RFC only affects platform build rules. All uses of removed categories will
be updated as part
of the implementation.

## Security considerations

This RFC does not change which ABI surfaces are present in the build or exposed
to developers.

## Privacy considerations

This RFC does not change which ABI surfaces are present in the build or exposed
to developers.

## Testing

The category tests in
[`//build/sdk/sdk_common/sdk_common_unittest.py`](/build/sdk/sdk_common/sdk_common_unittest.py)
will be updated to reflect each category change.

In addition, FIDL libraries in any of these categories should have
`<library_name>.api_summary.json` files in `//sdk/history`. This can be verified
for libraries added to `compat_test`. In addition, we can locally delete all
such files and ensure they are generated (that there are no missing files in
Git).

## Documentation

The main [SDK categories documentation] will be updated. Updates will also be
made to the [RealmFactory SDK requirements] reference to `partner_internal`.

## Drawbacks, alternatives, and unknowns

Most alternatives involve fewer options so as to reduce the amount of
understanding and decision making required of platform developers. For example,
combining `partner` and `prebuilt` because they have the same
compatibility requirements or even having a single bit - something is "in the
SDK" / requires compatibility or not. There have been informal proposals for
completely replacing the existing categories with two Booleans

Though such options would accomplish the goals of ensuring compatibility when
appropriate, they do not address some of the non-technical requirements. For
example, platform developers have expressed the desire to have compatibility
guarantees without the overhead of a full API calibration. (Perhaps in the
future this will be less of a concern.) Also, there is slightly more flexibility
when we know that the platform team owns all uses of an API. This is perhaps
especially relevant for FIDL protocols where the platform is the client (see
[RFC-0241]). Naming conventions have been suggested as a way to preserve some of
these properties. However, this could be more confusing and is harder to
enforce, such as in build rules.

The changes in this RFC are more incremental, continuing to use the concept of a
category string and existing enforcement mechanisms, which means they can be
implemented quickly. Making these changes does not preclude making such changes
in the future when we have more experience and better understand our
compatibility needs. In particular, we may need mechanisms for defining
different compatibility windows for APIs in the SDK.

Another alternative is to keep the single category `partner_internal`, possibly
with a new name, for the host tool and precompiled library use cases. However,
it seems easier to explain something by describing the use case than to find an
unambiguous name that covers both. Having separate categories also provides a
record of their use should we need to consider breaking compatibility in the
future. Separation also allows the support windows for each to diverge should
the platform and host tools have different ranges of compatible API levels.

## Prior art and references

- [RFC-0165] defined the current set of categories, which are documented in the
  [existing SDK categories documentation].
- This [API Council thread], which resulted in the definition of the
  `partner_internal` category, discussed a number of related issues including:
    - The need for stable APIs that are not in the SDK.
    - The desire to not expose some APIs for direct use to applications and the
      advantages of controlling all code that uses an API.
    - The observation that CTF test authors would have a similar use case.
    - A description of the new category as "internal-visible, partner-stable,"
      which provides context for the eventual name `partner_internal`.
    - The suggestion of `"prebuilts"` as a name for the new category.
        - There was some resistance to this name based on other meanings, but
          anecdotally, this term appears to have become more common when
          referring to static and shared libraries in the SDK.
- [Issue 328322682] contains an earlier evaluation of the existing SDK
  categories.
- [Issue 367760026] provides motivation for `compat_test`.
- The [RealmFactory SDK requirements] describe the requirements for RealmFactory
  FIDL protocols.


[RFC-0165]: /docs/contribute/governance/rfcs/0165_sdk_categories.md
[RFC-0241]: /docs/contribute/governance/rfcs/0241_explicit_platform_external.md
[SDK categories documented in RFC-0165]: /docs/contribute/governance/rfcs/0165_sdk_categories.md#implementation
[SDK categories documentation]: /docs/contribute/sdk/categories.md
[existing SDK categories documentation]: https://cs.opensource.google/fuchsia/fuchsia/+/main:docs/contribute/sdk/categories.md;drc=7b061d17cca1907a4cdd22793dd84091b339a1a2
[compatibility tests]: /docs/development/testing/ctf/overview.md
[test realm factory]: /docs/development/testing/components/test_realm_factory.md
[RealmFactory SDK requirements]: /docs/development/testing/components/test_realm_factory.md#realmfactory_sdk_requirements
[FIDL platform]: /docs/reference/fidl/language/versioning.md#concepts
[Supported]: /docs/concepts/versioning/api_levels.md#supported
[Sunset]: /docs/concepts/versioning/api_levels.md#sunset
[HEAD]: /docs/concepts/versioning/api_levels.md#head
[NEXT]: /docs/concepts/versioning/api_levels.md#next
[product owners]:  /docs/glossary/README.md#product-owner
[SDK history]: /sdk/history/README.md
[issue 328322682]: https://fxbug.dev/328322682
[issue 367760026]: https://fxbug.dev/367760026
[issue 369892217]: https://fxbug.dev/369892217
[issue 372986936]: https://fxbug.dev/372986936
[API Council thread]: https://groups.google.com/a/fuchsia.dev/g/api-council/c/Mi0vwsEUeCE
