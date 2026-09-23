# Release

A Fuchsia _release_ is a set of generated artifacts that are the output of the
Fuchsia project at a given point in time, published under a specific _release
version_ name. Each Fuchsia release implements the ABI for a set of API levels
and thus can run any component whose build-time target API level is in that set.

Note: To learn more about the Fuchsia release process, see
[Release process](#release-process).

A release consists of:

- The [Integrator Development Kit (IDK)][idk], a small set of libraries,
  packages, and tools required to build and run programs that target Fuchsia.
  - A small number of SDKs based on the IDK (for example, the [Bazel
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
confidential. Both the IDK and the OS binaries have both public and
confidential variants.

What unites them is that:

- They are all built exclusively from a single revision of the Fuchsia [source
  tree][source-tree] (specifically, the Google-internal version of
  [`integration.git`][integration.git]), and
- They are all published to their respective repositories under the same
  _release version_.

[^historical]: Some of these products are defined in the Fuchsia source tree
    for purely historical reasons, and will be moved out-of-tree to promote
    separation between product and platform.

## Release versions {#release-versions}

A _release version_ is a string of characters that names a release.

Each release version has a corresponding tag in the Google-internal version of
`integration.git`, which immutably points to the git commit from which the
release binary artifacts were built.

Versions are named `M.YYYYMMDD.R.C` (e.g. `2.20210213.2.5`) where:

*   `M` indicates the "[milestone](#milestone)" of the release.
*   `YYYYMMDD` is the date when the release's history diverged from the `main`
    branch.
*   `R` indicates the "release" version number. It starts at `0` and increments
    when multiple releases are created on the same date.
*   `C` indicates the "candidate" version number. It starts at `1` and increments
    when changes are made to a previous release on a _milestone release
    branch_.

## Release process {#release-process}

Fuchsia has the following types of releases:

* [Canary releases](#canary)
* [Milestone releases](#milestone)

### Canary releases {#canary}

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

As such, canary releases are appropriate for development and testing, but are
not recommend for production. For production use cases, _milestone
releases_ are recommended.

[^time-periods]: This document does not specify any particular release cadence.
    Time periods are named to provide an "order of magnitude" estimate for the
    frequency of various processes. Cadences will change as project and
    customer needs evolve.

### Milestone releases {#milestone}

Every few weeks, a _milestone release branch_ is created in both the
Google-internal version _and_ the public mirror of `integration.git`, starting
from the git commit for an existing "known good" canary release. Releases
originating from a milestone release branch are known as _milestone releases_
or _stable numerical releases_.

Milestones are numerically sequential, and often prefixed with an "F" when
discussed (as in, "the F12 release branch").

Once the F`N` milestone release branch has been cut, mainline development in
the Fuchsia source tree will continue on the `main` branch, working towards the
next F`N+1` release, and canary releases will have a version beginning with
`N+1`, as shown in the following diagram:

![Diagram with colored arrows representing Fuchsia milestones. F11 begins on
the main branch, but then branches off to become the F11 branch. After that,
the main branch is labeled F12, which again branches off,
etc.](images/milestones.png)

Note: For more information on release versions, see
[Release versions][release-versions].

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

## Development on `main` {#development-on-main}

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

[integration.git]: https://fuchsia.googlesource.com/integration/
[source-tree]: /docs/get-started/learn/build/source-tree.md
[ctf]: /docs/development/testing/ctf/overview.md
[workbench]: /docs/contribute/governance/rfcs/0220_the_future_of_in_tree_products.md
[product-bundle]: /docs/glossary#product-bundle
[idk]: /docs/development/idk/README.md
[bazel-sdk]: /docs/development/sdk/index.md
[RFC-0227]: /docs/contribute/governance/rfcs/0227_fuchsia_release_process.md
[release-versions]: #release-versions
