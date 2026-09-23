<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0227" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This RFC formalizes the existing process by which the Fuchsia platform is
released.

Throughout this RFC, the word "release" refers to the act of generating and
publishing various artifacts for consumption by developers further down the
software supply chain (for example, component authors or product owners). It
_does not_ refer to the act of delivering updates to Fuchsia-based products to
end users.

## Motivation

Fuchsia has had a functional release process for a few years now. However,
though Fuchsia releases and milestone release branches are visible to outside
observers, there has not yet been an RFC documenting and defining this process.

Earlier Fuchsia releases involved close coordination with our downstream
partners, but each passing milestone has been less special and more mundane
than the ones before it.

In order to continue this trend towards maturity, each Fuchsia developer needs
to understand their role in preventing our releases from breaking downstream
partners. As such, this RFC codifies the parts of the existing Fuchsia release
process that are most important for a platform contributor to understand.


## Stakeholders

_Facilitator:_ davemoore@google.com

_Reviewers:_

- billstevenson@google.com, release program management lead
- abarth@google.com, author of related [RFC-0002]
- atyfto@google.com, co-author and infra lead

_Consulted:_ aaronwood@google.com, chaselatta@google.com, sethladd@google.com

_Socialization:_ Conferred with experts to verify that it accurately reflects
the current state of the world.

[RFC-0002]: /docs/contribute/governance/rfcs/0002_platform_versioning.md

## Requirements

This RFC describes the release process as it exists today. It does not
prescribe any changes.

As stated above, this RFC _does not_ discuss delivery of software to end users.
The artifacts described in this RFC are delivered to developers and product
owners. They use those artifacts to build software that ultimately gets
delivered to end users via processes that are beyond the scope of this RFC.

## Design

A Fuchsia _release_ is a set of generated artifacts that are the output of the
Fuchsia project at a given point in time, published under a specific _release
version_ name.

It consists of:

- The [Integrator Development Kit (IDK)][idk], a small set of libraries,
  packages, and tools required to build and run programs that target Fuchsia.
  - A number of SDKs based on the IDK (for example, the [Bazel
    SDK][bazel-sdk]).
- The Operating System (OS) binaries, including the kernel, bootloaders,
  packages, tools, and other ingredients necessary to configure and assemble
  Fuchsia [product bundles][product-bundle].
  - A number of pre-assembled product bundles (for example, the [workbench]
    product)[^historical].
- Various other artifacts, such as documentation and [Compatibility Tests for
  Fuchsia (CTF)][ctf] builds.

There's no single archive that contains all of these artifacts; different
artifacts are produced by different builders and uploaded to different
repositories. Some artifacts are freely available online, and some are
confidential. (Indeed, both the IDK and the OS binaries have both public and
confidential variants.)

 What unites them is that:

- They are all built exclusively from a single revision of the Fuchsia [source
  tree][source-tree] (specifically, the Google-internal version of
  [`integration.git`][integration.git][^internal]), and
- They are all published to their respective repositories under the same
  _release version_.

[idk]: /docs/development/idk/README.md
[bazel-sdk]: /docs/contribute/governance/rfcs/0139_bazel_sdk.md
[product-bundle]: /docs/glossary#product-bundle
[workbench]: /docs/contribute/governance/rfcs/0220_the_future_of_in_tree_products.md
[ctf]: /docs/development/testing/ctf/overview.md
[source-tree]: /docs/get-started/learn/build/source-tree.md
[integration.git]: https://fuchsia.googlesource.com/integration/

[^historical]: Some of these products are defined in the Fuchsia source tree
    for purely historical reasons, and will be moved out-of-tree as soon as
    possible to promote product/platform separation.

[^internal]: These tags are only present in the internal `integration.git` due
    to technical limitations - one day we may start publishing these tags in
    the public mirror as well.

### Release versions

A _release version_ is a string of characters that names a release.

Each release version has a corresponding tag in the Google-internal version of
`integration.git`, which immutably points to the git commit from which the
release's binary artifacts were built.

Versions are named `M.YYYYMMDD.R.C` (e.g. 2.20210213.2.5) where

*   `M` indicates the "[milestone](#milestone)" of the release.
*   `YYYYMMDD` is the date when the release's history diverged from the `main`
    branch.
*   `R` indicates the "release" version number. It starts at 0 and increments
    when multiple releases are created on the same date.
*   `C` indicates the "candidate" version number. It starts at 1 and increments
    when changes are made to a previous release on a _milestone release
    branch_.

### Canary releases

A few times a day[^time-periods], a _canary_ release is created, based on the
last-known-good revision of the Fuchsia source tree. Concretely, a git tag is
applied to that revision in the Google-internal version of `integration.git`,
and various builders are triggered to build and publish the artifacts described
above. Canary releases do not get their own release branches, only tags.

Each canary release is only supported for a brief window - until the next
canary release. Bugfixes are not cherry-picked onto canary releases. (Put
another way: the "candidate" version number of a canary release should always
be "1".) If a bug is found in a canary release, a fix for that bug will be
applied to the `main` branch of the Fuchsia source tree. Clients impacted by
that bug should roll back to an earlier release and/or await a subsequent
canary release that includes the fix.

As such, canary releases are appropriate for development and testing, but may
not be appropriate for production. For production use cases, _milestone
releases_ are preferable.

[^time-periods]: This RFC does not mandate any particular release cadence. Time
    periods are named to provide an "order of magnitude" estimate for the
    frequency of various processes.

### Milestone releases {#milestone}

Every few weeks, a _milestone release branch_ is created in both the
Google-internal version _and_ the public mirror of `integration.git`, starting
from the git commit for an existing "known good" canary release. Releases
originating from a milestone release branch are known as _milestone releases_
or _stable releases_.

Milestones are numbered sequentially, and often prefixed with an "F" when
discussed (as in, "the F12 release branch").

Once the F`N` milestone release branch has been cut, mainline development in
the Fuchsia source tree will continue on the `main` branch, working towards the
next F`N+1` release, and canary releases will have a version beginning with
`N+1`, as shown in the following diagram:

![Diagram with colored arrows representing Fuchsia milestones. F11 begins on
the main branch, but then branches off to become the f11 branch. After that,
the main branch is labeled F12, which again branches off,
etc.](resources/0227_fuchsia_release_process/milestones.png)

Milestone releases share the `M.YYYYMMDD.R` part of their version with the
canary release on which they are based, and therefore only the "candidate"
version number `C` changes between releases built from a given milestone
release branch. Note that this means it's not always immediately obvious from a
version whether that release is a canary or milestone release (though a value
of `C` greater than 1 indicates that a release comes from a milestone release
branch).

Unlike canary releases, milestone releases are _supported_ for some number of
weeks after their creation. That is, improvements may be made to milestone
release branches after the release branch is cut. After each change to a
milestone release branch, a new milestone release will be published with a
larger "candidate" version number.

As a matter of general policy:

* Features are developed on `main`, not in milestone release branches.
* Changes made to milestone release branches land on `main` first, and then are
  cherry-picked onto the branch.
* Only changes that fix bugs (be they related to quality, security, privacy,
  compliance, etc) will be made to milestone release branches. We do not make
  changes that introduce new functionality to milestone release branches.

These policies are designed to minimize the odds that changes to milestone
release branches introduce new bugs, and thus the reliability and stability of
releases associated with a given milestone should improve over time. As such,
downstream customers are encouraged to eagerly adopt these new milestone
releases.

Under certain exceptional circumstances, we may need to stray from these
policies (e.g., a security fix may need to be developed on a confidential
branch to avoid publicizing a vulnerability before a fix is ready). Whether to
include a change on a milestone release branch is ultimately the release
manager's decision.

## Implementation

This RFC does not recommend any changes to the Fuchsia release process.

## Backwards Compatibility

The Fuchsia project promises that all artifacts within a single release (be it
canary or milestone) are compatible with each other. For example, if a
developer builds a component using SDK version `2.20210213.2.5`, we promise
that component will run on a Fuchsia device with OS version `2.20210213.2.5`.
If the component sees incorrect behavior from the Fuchsia platform under these
circumstances, that will be acknowledged as a bug.

Strictly speaking, as of this writing, Fuchsia does not offer a formal
guarantee of API or ABI compatibility between _any_ two releases - no existing
RFC discusses the matter. However, in practice, Fuchsia components built with
an SDK from one release frequently run on Fuchsia systems based on a different
release, and we still address issues discovered in those configurations.

A conditional promise of API and ABI compatibility across versions will be
described and ratified in a future RFC.

## Documentation

Much of the content of this RFC will be copied into a [separate
document][release-doc], so it can continue to evolve after the RFC is ratified.

[release-doc]: /docs/concepts/versioning/release.md

## Drawbacks, alternatives, and unknowns

Our current version naming scheme does not provide an obvious distinction
between canary releases and milestone releases. Milestone releases just look
like canary releases with a lot of cherry-picks applied.

The presence of `YYYYMMDD` in the version number is rather unique in software
and may be misleading. A casual observer may believe this to be a build or
release date, with builds for later dates being "newer", but this is not
necessarily correct for milestone releases. In some cases, a release like
`13.20230801...` could include cherry-picked fixes that `14.20230910...` does
not.

We could move to a version naming scheme closer to [Semantic
Versioning](https://semver.org), with canary releases being marked by a
[pre-release identifier](https://semver.org/#spec-item-9).

Likewise, the terms _canary_ and _milestone_ are non-standard. More common
terms that are somewhat analogous could be _preview_ and _stable_.

Technically speaking, these would not be very difficult changes to make, so we
may well want to improve these things in a subsequent RFC.

## Prior art and references

- Chromium [release channels][chromium-channels] and [version
  numbers][chromium-numbers].
- [semver.org](https://semver.org)

[chromium-channels]: https://www.chromium.org/getting-involved/dev-channel/
[chromium-numbers]: https://source.chromium.org/chromium/chromium/src/+/main:docs/website/site/developers/version-numbers/index.md