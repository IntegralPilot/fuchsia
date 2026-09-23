# Fuchsia SDK Contributor Guide

This section includes documentation for contributing to the [Fuchsia API
Surface][fuchsia-api-surface] and the [IDK].

_Technically_, it may be more appropriate to call this the "IDK Contributor
Guide", as the APIs and libraries that make up the API Surface are first added
to the IDK, which is then turned into an SDK distribution. However,
colloquially and in code, we almost exclusively say "the SDK". Regardless, if
you're looking to "change the SDK" in some way, you're probably in the right
place.

## Contributing to an API in the SDK

To contribute to the [Fuchsia API Surface][fuchsia-api-surface], do the
following:

* Evaluate whether your change is large or small.

  * If you have a small, incremental change to the API, contribute your
      change by completing the steps in
      [Create a change in Gerrit][create-a-change-in-gerrit], as you would for
      any Fuchsia source code change.
  * If you have a large change to the API, that is, a change that
      significantly expands on the function of the API or modifies the
      API extensively, do the following:

    * Create an [RFC][rfc] that explains the design of your modification
         to the API.
    * This RFC should be reviewed through the normal
      [RFC process][rfc-process]. The API reviewer for the relevant area should
      be a stakeholder in the RFC. See the
      [Fuchsia API Council Charter][api-council] to identify API reviewers.
    * After your API RFC is approved, contribute your change by completing the
      steps in [Create a change in Gerrit][create-a-change-in-gerrit], as you
      would for any Fuchsia source code change.

* [Request a code review][request-a-code-review] from an API council member.
  Select your API council reviewer based on the area of the Fuchsia API that
  you're modifying. For a list of API council members and their areas of focus,
  see [Membership][membership] in the Fuchsia API Council Charter.


## Promoting an API {:#promoting-an-api}

By default, APIs, libraries, etc. are not IDK atoms, not in the IDK/SDK, and not
in an [SDK category][sdk-category]. In order for an API or library to be used
in certain ways, including inclusion in the IDK, it must be made an IDK atom and
assigned an SDK category.

This section describes the process for promoting an atom to the different
categories. These apply to both adding a new atom to a category and promoting
an existing atom from one category to another.

In order for an atom to be added to the IDK/SDK, it must be [promoted to the
`partner` category](#promoting-to-partner).

<a id="promoting-to-host-tool"></a> {#  A second anchor for this section. #}
### Promoting an API to the `prebuilt` or `host_tool` category {:#promoting-to-prebuilt}

For an API to be usable by prebuilt binaries or host tools in the Fuchsia SDK,
it must be in the `prebuilt` or `host_tool` [SDK category][sdk-category],
respectively. The API must follow the
[API evolution guidelines][evolve-gracefully] which are focused on enabling API
evolution while maintaining compatibility.

Keep in mind that there are additional considerations when
[promoting an API to `partner`](#promoting-to-partner), which may reveal
tradeoff decisions between compatibility and long term usability.

Once the API is ready for review, request an [API calibration][calibration].
Note that API calibration is targeted at *stable* API surface. Category
promotions of libraries only available at `HEAD`, which may occasionally be
necessary when the library is a dependency of another library, may defer
calibration until the API is stabilized.

### Promoting an API to the `partner` category {:#promoting-to-partner}

For an API to be included in the Fuchsia IDK/SDK, it must be in the `partner`
[SDK category][sdk-category]. This requires clearing two hurdles: there must be
a ready and willing customer, and the API must have gone through [API
calibration][calibration]. Note that API calibration is targeted at *stable*
API surface. Promotions to `partner` where the library is marked `stable =
false` do not modify the stable API surface and may defer calibration until the
API is stabilized.

<!-- Reference links -->

[fuchsia-api-surface]: /docs/glossary/README.md#fuchsia-api-surface
[IDK]: /docs/development/idk/
[create-a-change-in-gerrit]: /docs/development/source_code/contribute_changes.md#create-a-change-in-gerrit
[request-a-code-review]: /docs/development/source_code/contribute_changes.md#request-a-code-review
[rfc]: /docs/contribute/governance/rfcs/TEMPLATE.md
[rfc-process]: /docs/contribute/governance/rfcs/rfc_process.md
[api-council]: /docs/contribute/governance/api_council.md#area
[membership]: /docs/contribute/governance/api_council.md#membership
[sdk-category]: /docs/contribute/sdk/categories.md
[calibration]: /docs/contribute/governance/api_council.md#calibration
[evolve-gracefully]: /docs/development/api/evolution.md#evolve-gracefully
