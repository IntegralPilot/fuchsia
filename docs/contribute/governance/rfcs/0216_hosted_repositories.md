<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0216" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

With the advent of [full Bazel support for Fuchsia][RFC-0139] and the increasing
focus on a fully featured Fuchsia SDK, the advantages of developing Fuchsia
components inside of `fuchsia.git` have significantly diminished.  This RFC
outlines requirements and best practices for developing components in other
Fuchsia hosted repositories.  It does not address the needs of assembly outside
of `fuchsia.git`.

## Motivation

Since the inception of the Fuchsia project, most development has taken place in
a single environment managed by the [`jiri`
tool](https://fuchsia.googlesource.com/jiri/), which coordinates development in
multiple source code repositories as if they were a single repository.  Today,
we informally refer to that environment as `fuchsia.git`, as most of Fuchsia's
development is centered in [that
repository](http://fuchsia.googlesource.com/fuchsia).

A single managed environment, mostly in a single repository, has significant
advantages for development.  Code can be updated quickly across the codebase,
typically in a single change, reducing the need for complicated coordination
between versions in different repositories.  Updates to developer tools and
third party / library code can be centralized, which means that the entire
organization can share the benefits of upgrades and code cleanup.  Common code
standards remove the need for arguments about code style or directory layout.
Developers know what repository to search for any given code.

However, a centralized repository does not meet the needs of all Fuchsia
developers.  Fuchsia's repository requires developers to build entire system
images and SDKs, significantly impacting developer velocity.  The need to build
system images and SDKs in fuchsia.git means that, for many developers, the
experience building is very different.

Perhaps more importantly, the Fuchsia team creates abstractions for developers
who do not work in `fuchsia.git`, but we often don't use them ourselves.  This
creates a problem for developers who start working in `fuchsia.git` expecting
standard Fuchsia workflows, but instead encounter the `fx` tool and a variety of
scripts that are almost, but not quite, completely unlike what we share with
SDK-based developers.  It also makes it much harder for `fuchsia.git` based
developers to empathize with SDK based developers - because they don't use the
same workflows, they do not have the same incentives to make those workflows
great.

The Fuchsia Build Team is working to resolve some of the differences between
`fuchsia.git`-based and SDK-based workflows, but some gap will always remain, if
only because developers using SDK-based workflows in other repositories never
have to build the SDK or system images as part of those workflows.

We do not expect all developers to move their code to SDK-based workflows in the
near term.  In 2023, the team is focusing on support for migrating drivers out
of `fuchsia.git`.  For other projects, moving out of `fuchsia.git` is optional.
In the near term, we expect mostly components that can be built entirely on
SDK-based API to be migrated out of fuchsia.git.

To increase developer productivity for projects that do not have to contribute
to the SDK, and to increase empathy for SDK-based developers on the Fuchsia
team, the Fuchsia community wants to encourage developers to use SDK-based
workflows outside of `fuchsia.git`.  However, we do not have clear guidelines
for how to do so.  This RFC, based on our experience with SDK-based workflows
and `fuchsia.git` workflows over the past few years, exists to start to fill in
those gaps.

## Stakeholders

_Facilitator:_

* David Moore (davemoore@google.com)

_Reviewers:_

* Adam Barth (abarth@google.com) - FEC
* Chase Latta (chaselatta@google.com) - Bazel support
* Nathan Mulcahey (nmulcahey@google.com) - EngProd
* Suraj Malhotra (surajmalhotra@google.com) - Drivers

_Consulted:_

The following additional people were consulted on this RFC:

* Andres Oportus (andresoportus@google.com)
* Christopher Anderson (cja@google.com)
* Clayton Wilkinson (wilkinsonclay@google.com)
* Danny Rosen (dannyrosen@google.com)
* Harsh Priya NV (harshanv@google.com)
* Janna Shaftan (jshaftan@google.com)
* John Wittrock (wittrock@google.com)
* Renato Mangini Dias
* Marc-Antoine Ruel (maruel@google.com)
* Nico Haley (nicoh@google.com)
* Niraj Majmudar (nirajm@google.com)
* Seth Ladd (sethladd@google.com)
* Tim Kilbourn (tkilbourn@google.com)

_Socialization:_

A short version of this RFC was socialized with the EngProd, FHCP, Drivers, Eng
Council, Bazel SDK Integration, and DevRel teams.

## Design

The following is a set of guidelines for starting and developing SDK-based,
Fuchsia-hosted repositories.  They are not intended to be exhaustive.  For
example, while we lay out some guidelines for how to keep SDKs in Fuchsia hosted
repositories updated, we do not have a detailed design for that process.

### Repository Naming

Repository names depend on the content of the repository.  We break down the
guidance into driver and non-driver repositories.

#### Drivers

Driver code that is not specific to a particular vendor shall be hosted in
repositories named after the driver area (e.g., drivers/audio.git,
drivers/wlan.git).  This includes, for example, code for generic drivers, like
keyboards and mice.  Closed source code should also not go into these
repositories.

We recognize that the names of these areas may change over time.  Moving a
repository to a new path can be difficult.  We encourage developers to choose
the name of their area carefully, and to ensure that due diligence is done when
changing names later.

Driver code that is specific to a particular vendor shall be hosted in
drivers/<area>/<vendor>/<drivername>.  For example,
[drivers/wlan/intel/iwlwifi].  Teams have flexibility about where the
repository boundaries are (for example, whether `drivers/wlan/intel/iwlwifi` is
a separate repository, or whether it goes in the `drivers/wlan` repository).  We
can provide some guidance based on our existing experience; we believe we will
develop more experience with this over time:

- If a driver requires closed-source software to build, we recommend keeping it
  in a different repository from fully open sourced drivers.

- If multiple drivers share substantial code (see code sharing below), keeping
  them in a single repository with that code avoids the difficulty of rolling
  out updates to that code across multiple repositories.  Developers in such
  situations often find it difficult to locate all of the relevant repositories,
  build the software in them, and test them.  For most code, fewer repositories
  means less work in keeping up with the latest updates to libraries, SDKs, and
  toolchains.

- Sometimes, multiple drivers share substantical code, but circumstances dictate
  keeping them in separate repositories.  Gerrit support for repos with name
  overlap (e.g, if `drivers/<area>` and `drivers/<area>/<vendor>` are both
  repos) is awkward, and the overlap creates the possibility of accidental
  collisions when developers want to check both repositories out into the same
  directory.  We recommend placing common code in a repository with the name
  `drivers/<area>/common` instead of one called `drivers/<area>`.  Code may be
  developed in common areas before multiple drivers need it, to avoid the costs
  of code migration.

- Driver teams may decide to release their drivers for certification off of a
  stabilization branch.  If they do so, they might find it easier to manage one
  branch than multiple branches.  The Fuchsia team is developing best practices
  for driver certification, which is likely to be a topic of a future RFC.

#### Non-driver components

Component source code that is unrelated to drivers may be hosted in one of two
locations.  Developers have flexibility about the choice.

A component may be hosted in a repository named after the [functional
area](/docs/contribute/governance/api_council.md#area)
that sponsors that component (e.g., experiences.git).  We recommend this for
components that are not likely to be of interest to developers outside of the
functional area.

A component may be hosted in a repository named after the project (e.g.,
workstation.git, cobalt.git).  We recommend this for larger projects and
projects that may be of interest to developers outside of the functional area.

As with driver code, there are pros and cons to shared source bases.  The
considerations described in that section apply to components, as well.

### Repository Layout

The code layout shall be the same as it is in the [Fuchsia source code layout
document](/docs/development/source_code/layout.md).  The
source code layout document must be updated to reflect allowable use of the
Bazel build system; i.e., allowing BUILD.bazel instead of BUILD.gn, and the
presence of WORKSPACE files.

There will be a base template repo that developers can clone to make a new repo.
We may have additional repos with tools and layout specialized for drivers,
components, and, if we develop the need for it, product integration.  The
template will contain required basics for a Fuchsia repository.  This currently
includes, for example, a LICENSE file, a Bazel build file with pre-defined
targets that infrastructure can read to know what to build and test, and
top-level OWNERS file with at least two owners listed.

Drivers require certification tests.  The certification program is called the
Fuchsia Hardware Certification Program, or FHCP.  FHCP tests that are hosted in
`fuchsia.git` shall be in the top level directory called `tests/conformance` in
the relevant source area.  Note that the top level directory of a source area is
_not_ necessarily the same thing as the repository root directory, so
`tests/conformance` directories in `fuchsia.git` do not necessarily have to be
located in the repository root.  FHCP tests that are hosted in other
Fuchsia-hosted SDK-based repositories must be under a directory called
`tests/conformance` in the top level source area for the repository
corresponding to the relevant driver area (e.g., `drivers/wlan.git`,
`drivers/audio.git`).

### Code Sharing

The Fuchsia organization supports several methods of sharing code between
repositories.

#### Sharing via the Fuchsia SDK

The primary mechanism for code sharing between Fuchsia hosted repositories is
the Fuchsia SDK.  The Fuchsia SDK is used to develop software that runs on
Fuchsia products.  The parts of the SDK that are developed by the Fuchsia
organization (excluding, for example, the emulator) are currently developed
exclusively in `fuchsia.git`, although this may change over time.

The Fuchsia SDK is composed of multiple software artifacts, or _elements_.
[RFC-0165] describes several SDK categories; each element is assigned one of
these categories.  The categories roughly describe how stable we consider each
artifact to be, and where we think they can appropriately be used.  Three of
these categories are relevant to this RFC:

 - `internal`: supported for use within the Fuchsia platform source tree;
 - `partner`: supported for use by select partners;
 - `public`: supported for use by the general public.

Fuchsia developers may be familiar with the [IDK](/docs/development/idk), which
currently consists of a group of partner SDK elements.

Because of their proximity to the platform, we primarily expect Fuchsia-hosted,
SDK-based repositories to use partner and internal category SDK elements.
Whether they use partner or internal depends on their use case:

- Fuchsia hosted drivers should be developed using partner category SDK elements
  that are delivered out of `fuchsia.git`.  This includes, for example,
  everything that can be found in the IDK.

- Fuchsia hosted components that are downstream consumers of the SDK should be
  developed using partner category SDK elements that can be delivered out of
  `fuchsia.git`.

- Fuchsia hosted components that implement partner category SDK interfaces may
  use internal SDK elements.

In cases where developers need to share platform API or ABI, they should use the
Fuchsia SDK.  In particular, FIDL definitions are interfaces to the platform,
and should be shared via the SDK.  If a developer believes that there is an
advantage to sharing their FIDL definition another way, they should consult with
the Fuchsia API Council.

#### Sharing via git submodules

The sharing of code via git submodules is a common pattern in `fuchsia.git`.  A
git submodule is a git project used within another project.  The advice in this
section is intended to apply to both repositories managed by the `jiri` tool and
git submodules.

There are a number of cases where it is inappropriate to share code using an
SDK.  We do not want to redistribute most third party code via the SDK, because
then it is incumbent on us (rather than the third party) to support that code
for all SDK users.  We also have libraries that are intended for use exclusively
by the Fuchsia team, such as [fbl].  These libraries may be included in
the internal SDK, but may also be acceptable for use by Fuchsia teams that use
the partner SDK.

In addition, there are a number of tools that are used for engineering
productivity purposes but are specific to Fuchsia's infrastructure and code
standards, such as the code formatter and the various fx tools that query
gerrit.

Shared code that should not be in the partner SDK may be shared by migrating it
to a git submodule and sharing it between repositories that need it.  The API
council and the RFC process arbitrate what can be included in the partner SDK.

We may use this mechanism for code currently stored in `fuchsia.git` that we
want to share among Fuchsia organization developers, but do not want to put in
the SDK, like the `fbl` or `boringssl` libraries.

#### Sharing via binary

Some tools may be shared via binary.  For example, we use CIPD to make copies of
our toolchain available to `fuchsia.git` based developers.  We will continue
this practice.  Best practices are the same as they are for git submodules
above: developers are encouraged to keep up to date with the latest tools
available for their repository.

#### Staying up to date {#updating}

It is critical for Fuchsia hosted repositories to stay up to date with the
latest published software.

- All repositories must use autorollers that update their dependencies on a
  daily (or faster) cadence.

- As described in [RFC-0148], each project *should* attempt to
  release and roll their own contents at a fast cadence.  That RFC does not
  define what a fast cadence is, although it suggests the ideal state is one
  where new dependencies are rolled within hours of release.  We leave the
  interpretation of fast cadence to the repository owners, with the following
  caveats.

- All repositories *must* use an SDK that is within the _compatibility window_.
  The _compatibility window_ for an SDK is the length of time that code built
  with that SDK will work with Fuchsia products built from head.

- Repositories under active development - where the SDK they use is actively
  changing to better support the use cases in that repository - *should* update
  their SDKs daily (adjusting for the time it takes to resolve
  incompatibilities, broken infrastructure, and other reasonable difficulties).

The Fuchsia organization will create a process for performing updates / LSCs
across all Fuchsia-hosted repositories.  Each repository owner must provide a
point of contact who will respond quickly to urgent update requests (e.g., for
security issues, fixes for toolchain updates, and so on).  This process should
allow developers making large scale change to track their changes over time.

All repository owners *must* respond promptly to requests to update for security
or compatibility reasons.

#### Multi-repository development

Although all checked-in code will need to be shared using one of the above
mechanisms, we expect developers will have use cases for which it makes sense to
be changing code in multiple repositories at the same time.  For example, a
developer may need to develop new FIDL APIs in `fuchsia.git` intended for use by
a driver in an SDK based workflow, and be working on the code for both at the
same time.

The Fuchsia team has experience making multi-repository workflows function.  For
the most part, they fall into one of the following categories:

- A developer working in `fuchsia.git` makes a change to the SDK, which they
  then export for use in another (destination) repository.

- A developer working on a component in one source repository needs to publish
  that component to a package repository hosted in another (destination)
  source repository.

- A developer working on a component in one source repository needs to make that
  component available for system assembly in another (destination) source
  repository.

The Fuchsia team will develop tools that make it easy to support these
workflows.

### Build system

Fuchsia hosted repositories must use the Bazel build system.  Fuchsia's
commitment to Bazel for new repositories is laid out in detail in [RFC-0095] and
[RFC-0139], and does not need to be reiterated here.

### Product Integration

Because of the limited number of product builds we support, we will not require
special product integration repositories for products at this time.  The
Workstation product will continue to perform assembly in `workstation.git`
(**2023Q3 note**: this plan to do assembly in `workstation.git` was deprecated
by [RFC-0220]).

## Implementation

This RFC gives high-level guidelines on how projects should lay out and manage
their repositories, but is intentionally light on implementation details. Each
project may follow the guidelines in a number of ways, and we don't want to
create artificial constraints by prescribing too many specifics. New
Fuchsia-hosted SDK-based projects are still starting, and anything we map out
here is likely to go stale as the projects evolve.

## Performance

We do not believe that the presence of multiple repositories will have a
meaningful impact on performance.

Currently, because all code in `fuchsia.git` is built from head with the same
dependencies and toolchain configuration, shared code among different components
is bit-for-bit identical.  Fuchsia's file systems use this feature to
deduplicate binaries, thereby saving space on target devices.  Splitting code
among multiple repositories causes version skew among toolchains and
dependencies, potentially resulting in different versions of the same binary
being deployed to the device, costing space.  This can be mitigated by trying
hard to keep the same SDKs and toolchains across all repositories (see the
[Staying up to date](#updating) section above).

## Ergonomics

As described in the motivation section, we believe the increased use of Bazel
and the SDK will result in faster development cycles for driver and component
developers.

## Backwards Compatibility

The chances of backwards incompatibilities increase as repositories let their
shared code go out of date.  We therefore provide the recommendations in the
[Staying up to date](#updating) section.

## Security considerations

We do not believe that Fuchsia hosted repositories will create security risks.

## Privacy considerations

We do not believe that Fuchsia hosted repositories will create privacy concerns.

## Testing

We do not believe substantial new testing is required.  Fuchsia infrastructure
still runs all relevant tests, regardless of source location.

## Documentation

We will create how-to guides and references for starting new repositories.  We
will also create example repositories that can be cloned easily (these already
exist, to some extent).

## Drawbacks, alternatives, and unknowns

We are promulgating this RFC in advance of significant development being done in
Fuchsia-hosted, SDK-based repositories.  Many of the standards here are best
guesses based on experience, and may not meet our needs.  We will likely revisit
the guidance in this document as we gain more experience with these workflows,
and as we encourage more code to be developed outside of `fuchsia.git`.

[RFC-0095]: /docs/contribute/governance/rfcs/0095_build_and_assemble_workstation_out_of_tree.md
[RFC-0139]: /docs/contribute/governance/rfcs/0139_bazel_sdk.md
[RFC-0148]: /docs/contribute/governance/rfcs/0148_ci_guidelines.md#consider_fast_roll_and_release_cadences
[RFC-0165]: /docs/contribute/governance/rfcs/0165_sdk_categories.md
[RFC-0220]: /docs/contribute/governance/rfcs/0220_the_future_of_in_tree_products.md

[fbl]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/system/ulib/fbl/
[drivers/wlan/intel/iwlwifi]: https://fuchsia.googlesource.com/drivers/wlan/intel/iwlwifi/
