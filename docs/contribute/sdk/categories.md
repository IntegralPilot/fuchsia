# SDK categories

Each [SDK Atom] has a category that defines which kinds of SDK consumers can see
the Atom. As SDK Atoms mature, we can increase their visibility, which implies
increasing their stability guarantees.

[SDK Atom]: /docs/glossary#sdk-atom

## Motivation

Fuchsia is built by combining many different components that interact using
protocols with schemas defined in FIDL. Components that are part of the Fuchsia
project interact with each other using the same mechanism that components
written by third parties interact with the Fuchsia platform. For this reason,
we benefit from having a uniform mechanism that can be used both to develop
Fuchsia and to develop for Fuchsia.

The simplest approach would be to put all the FIDL definitions into the Fuchsia
SDK, and then have all the developers use those same FIDL definitions in
developing their components. However, this approach breaks down because of a
common tension in designing APIs: API designers need the ability to iterate on
their designs and API consumers need stability in order to build on top of the
APIs.

This document describes SDK categories, which is Fuchsia's primary mechanism
for balancing these concerns.

## Design

FIDL libraries are one example of an SDK Atom, but there are other kinds of
SDK Atoms, including C++ client libraries, documentation, and tools. SDK
categories apply to every kind of SDK Atom, but this document uses FIDL
libraries as a running example.

SDK categories balance the needs for iteration and stability in APIs by
recognizing that different API consumers have different stability needs. API
consumers that are "closer" to API designers typically have less need for
stability and often are the first customers that provide implementation
feedback for API designers.

Each SDK Atom is annotated with an SDK category, which defines which SDK
consumers can depend upon the SDK Atom. For example, if the `fuchsia.foo` FIDL
library has an SDK category of `compat_test`, that means only CTF tests within
the Fuchsia project can depend upon `fuchsia.foo`. If someone wants to change
`fuchsia.foo`, they run the risk of breaking CTF tests inside the Fuchsia
project but they do not run the risk of breaking consumers in other projects.

As another example, consider a `fuchsia.bar` FIDL library with an SDK category
of `partner`, which means `fuchsia.bar` can be used both within the Fuchsia
project and by SDK consumers who have partnered[^1] with the Fuchsia project.
When someone changes `fuchsia.bar`, they run a larger risk of breaking
consumers because they might break the partners that depend upon `fuchsia.bar`.

An additional type of SDK category is required for the APIs used in the prebuilt
`partner` SDK atoms when it's undesirable to expose these APIs to
SDK users. This `prebuilt` category will enforce
the same API compatibility windows as the `partner` category
without requiring adding those APIs to the SDK API surface area.

A typical SDK Atom begins its lifecycle outside the SDK with no SDK category. It
may first be added to the SDK in the `host_tool` or `prebuilt` category when a
host tool or precompiled library, respectively, in the SDK needs to use it. At
some point, the API Council will promote the SDK Atom to the `partner` SDK
category, often when a partner needs access to an API contained in the Atom.

Please note that this mechanism is complementary to `@available` mechanism for
[platform versioning][fidl-versioning]. The `@available` mechanism *records*
when and how FIDL APIs change. The SDK category mechanism determines the
*policy* for how quickly API designers can make changes.

[^1]: Currently, the set of partners is not public. As the project scales, we
      will likely need to revisit our approach to partnerships.

[fidl-versioning]: /docs/reference/fidl/language/versioning.md

## Categories

SDK categories have been implemented in the [`sdk_atom`](/docs/glossary#sdk-atom) GN Rule.
Each SDK Atom has an `category` parameter with one of the following values:

- `compat_test` : May be used to configure and run CTF tests but may not be
                  exposed for use in production in the SDK or used by host
                  tools.
- `host_tool`   : May be used by host tools (e.g., ffx) provided by the platform
                  organization but may not be used by production code or
                  prebuilt binaries in the SDK.
- `prebuilt`    : May be part of the ABI that prebuilt binaries included in the
                  SDK use to interact with the platform.
- `partner`     : Included in the SDK for direct use of the API by out-of-tree
                  developers.


These categories form an ordered list with a monotonically increasing audience.
For example, an SDK Atom in the `partner` category is necessarily available to
Compatibility Tests `partner` comes after `compat_test` in this list.


For information on promoting an API, library, or existing SDK atom to one of
these categories, see [Promoting an API][sdk-category-promotion].

[sdk-category-promotion]: /docs/contribute/sdk/README.md#promoting-an-api

## Commitments

Adding an API to the `partner` or `prebuilt` category amounts to a
commitment to our partners that we will not break their code or impose undue
[churn][churn-policy] on them. Each team that owns an API in one of these
categories has a responsibility to uphold these commitments.

[churn-policy]: /docs/contribute/governance/policy/churn.md

### `host_tool`

Partners don't write their own code using `host_tool` APIs or develop
components that depend on them, but they still depend on these APIs _indirectly_
via developer tools written by the Fuchsia team. Since the Fuchsia team owns the
code that uses these APIs, we can change these APIs without churning our
partners. However, the tools that use
`host_tool` APIs will, in general, be built from a different revision
than the platform components that they talk to. Thus, we must follow our ABI
compatibility policies whenever we change `host_tool` APIs.

Namely, the owners of an API in the `host_tool` category agree to:

* Use [FIDL Versioning][fidl-versioning] annotations on their APIs.
* Only ever modify their API at `HEAD` or `NEXT`.
  Once an API level is declared stable, it should not be changed (see [version_history.json]).
* Keep the platform components that implement those APIs compatible with all
  Fuchsia-supported API levels (see [version_history.json]).

See the [API evolution guidelines][evolution-guidelines] for more details on
API compatibility.

[version_history.json]:  /sdk/version_history.json
[evolution-guidelines]: /docs/development/api/evolution.md

### `prebuilt`

Partners don't write their own code using `prebuilt` APIs, but they
still depend on these APIs _indirectly_ via prebuilt libraries or packages
written by the Fuchsia team. Since the Fuchsia team owns the code that uses
these APIs, we can change these APIs without churning our partners. However,
the libraries and packages that use `prebuilt` APIs will, in
general, be built from a different revision than the platform components that
they talk to. Thus, we must follow our ABI compatibility policies whenever we
change `prebuilt` APIs.

Namely, the owners of an API in the `prebuilt` category agree to:
* Make all the versioning commitments from the `host_tool` section above.

See the [API evolution guidelines][evolution-guidelines] for more details on
API compatibility.

[version_history.json]:  /sdk/version_history.json
[evolution-guidelines]: /docs/development/api/evolution.md

### `partner`

Partners use `partner` APIs directly. These APIs are the foundation on which
our partners build their applications, and it is our responsibility to keep
that foundation reliable and stable.

Owners of an API in the `partner` category agree to:

* Make all the versioning commitments from the `prebuilt` section above.
* Own our partners' developer experience when it comes to this API, including:
  * Providing good documentation.
  * Following consistent style.
  * Anything else you'd like to see in an SDK you were using.

  See the [API Development Guide][api-dev] for relevant rules and suggestions.
* Acknowledge that backwards-incompatible changes to new API levels impose a
  cost on our partners, even when we follow our [API evolution
  guidelines][evolution-guidelines]. If and when a partner chooses to update
  their target API level, they will need to make modifications within their
  codebase to adapt to your change. As such, these changes should not be made
  lightly.

  If you _do_ decide a backwards-incompatible change is worth making, you agree
  to pay most of the downstream costs of that change, in accordance with the
  [churn policy][churn-policy].

  Changes are _much_ easier to make before APIs are stabilized, so
  any planned API refactoring should be done just _before_ adding an API to the
  an SDK category, rather than after.

[api-dev]: /docs/development/api/README.md

## Change history

- First documented in [RFC-0165: SDK categories][rfc-0165].

[rfc-0165]: /docs/contribute/governance/rfcs/0165_sdk_categories.md
