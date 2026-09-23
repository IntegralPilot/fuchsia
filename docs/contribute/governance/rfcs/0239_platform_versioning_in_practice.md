<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->

{% set rfcid = "RFC-0239" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}

# {{ rfc.name }}: {{ rfc.title }}

{# Fuchsia RFCs use templates to display various fields from \_rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}

<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

The goals of this RFC are to expand upon [RFC-0002], [RFC-0083], and others by:

- laying out a conceptual model for how API levels and ABI revisions are used
  in practice on Fuchsia,
- providing API and ABI compatibility guarantees to our partners, stated in
  terms of that conceptual model, and
- outlining how we plan to meet those compatibility guarantees.

Briefly summarized:

- Each Fuchsia [milestone] has an accompanying _API level_. API level 15 can be
  thought of as "the 15th edition of the Fuchsia platform surface, which was
  first supported by F15."
- Each Fuchsia [release] supports _multiple_ API levels. For example, F15
  milestone releases supported API levels 11, 12, 13, 14, and 15.
- End-developers select a single _target API level_ when building a component.
  A component targeting API level 13 will see the Fuchsia platform surface _as
  it was_ in API level 13, and will run on any Fuchsia system that supports API
  level 13.
- The Fuchsia kernel and platform components support multiple API levels by
  implementing them all _simultaneously._

[rfc-0002]: /docs/contribute/governance/rfcs/0002_platform_versioning.md
[rfc-0083]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md
[milestone]: /docs/concepts/versioning/release.md#milestone
[release]: /docs/concepts/versioning/README.md#releases

## Motivation

As discussed in [RFC-0227], the primary components of a Fuchsia platform
release are:

- The [Integrator Development Kit (IDK)][idk], a small set of libraries,
  packages, and tools required to build and run components that target Fuchsia,
  and
- The Operating System (OS) binaries, including the kernel, bootloaders,
  packages, tools, and other ingredients necessary to configure and assemble
  Fuchsia product bundles.

The IDK is the build-system-agnostic precursor to the Fuchsia SDKs, describing
the API elements (e.g., protocols, methods, fields, arguments, libraries) that
make up the Fuchsia platform surface and allowing end-developers to write
components that use those API elements. Some of the interfaces in the IDK are
implemented by OS binaries like the kernel and platform components, and others
are implemented by external components (for instance, applications or drivers),
with the expectation that the OS binaries will call them.

The Fuchsia platform surface is under constant development, and the OS binaries
are constantly updated to match. These changes quickly[^canaries] flow
downstream to end-developers and product owners. Before the introduction of API
levels, the IDK only contained a single description of the platform surface. An
IDK built on Monday would describe a different version of the Fuchsia platform
surface from the one implemented by an OS built later that week on Friday. This
used to cause issues: if a FIDL method and its implementation were deleted on
Wednesday, an external component built with Monday's IDK may try to call that
method, only to be told by Friday's OS that no such method exists.

Naively, we could try to forbid mismatches between the IDK and OS versions. We
could say that the OS will only run components built using the IDK from the
exact same Fuchsia release (that is, OS version `2.20210213.2.5` would only run
components built with IDK version `2.20210213.2.5`). However, this is
infeasible - we can't require end-developers to constantly update and rebuild
their components as we release new versions of Fuchsia. Thus, in general, the
IDK that builds external components and the OS that runs them may come from
different Fuchsia releases - possibly even from different milestones.

This prompts the question: under what circumstances will a component built with
an IDK from one release be able to successfully run on the OS from another?

[RFC-0002] introduced API levels and ABI revisions as tools to help answer that
question, but wasn't very specific about how they ought to be used in practice,
and provided no concrete compatibility guarantees. This RFC fills those gaps by
showing how API levels and ABI revisions are used in practice, establishing a
compatibility guarantee policy, and briefly discussing strategies for complying
with that policy.

[^canaries]: Most of our downstream partners currently depend on canary
    releases of Fuchsia, which are produced multiple times per day. In the
    future, more of our partners will consume the IDK and OS binaries from
    milestone release branches, but for now, we value the immediate feedback
    this arrangement allows.

[rfc-0227]: /docs/contribute/governance/rfcs/0227_fuchsia_release_process.md
[idk]: /docs/development/idk/README.md

## Stakeholders

_Facilitator:_ jamesr@google.com

_Reviewers:_

- abarth@google.com
- ianloic@google.com
- mkember@google.com
- yaar@google.com

_Consulted:_

- chaselatta@google.com
- ddorwin@google.com
- sethladd@google.com

_Socialization:_

The model and policies in this RFC have been built out incrementally over the
past few years, but have not thus far been encoded in any RFC. This particular
proposal was arrived at through many docs and conversations, largely taking
place between members of the Platform Versioning Working Group.

## Assumptions

When describing compatibility between a component that exposes an API and
another component that uses that API, we assume that one side of the
interaction is an external component built [out-of-tree][out-of-tree] (that is,
outside of the Fuchsia platform build, via the IDK), and the other side of the
interaction is an OS binary, like the kernel or a Fuchsia platform component,
built [in-tree][in-tree] (that is, built within the Fuchsia platform's GN build
graph). Defining versioning practices and compatibility for interactions
_between_ external components is out of scope and left for future work.

[out-of-tree]: /docs/glossary#out-of-tree
[in-tree]: /docs/glossary#in-tree

## Design

This section will lay out the conceptual model behind Fuchsia Platform
Versioning. Many of the concepts described here were previously introduced in
[RFC-0002], but are presented here with more concrete detail and restrictions.

The text of this "Design" section should be taken as informative, rather than
normative. The [Policy changes](#policy-changes) section below will repeat any
parts of this text that represent changes from existing policy.

### API levels

The [Fuchsia platform surface][platform-surface] is constantly changing, and
new Fuchsia [releases][rfc-0227] are constantly being created - multiple times
a day. Rather than reasoning about each Fuchsia releases's compatibility with
the Fuchsia platform surface from each _other_ release, which would be
overwhelming, we think about a release's compatibility with a limited number of
**API levels**.

Think of API levels as "versions", "editions", or "snapshots" of the APIs that
make up the Fuchsia platform surface. API level 10 is "the 10th edition of the
Fuchsia API surface." API levels are synchronized with milestones, so API level
10 can also be thought of as, "the latest version of the Fuchsia platform
surface when F10 was published."

You can use API levels to make statements like, "the
`fuchsia.lightsensor.LightSensorData` FIDL table had three fields in API level
10 (`rgbc`, `calculated_lux`, and `correlated_color_temperature`), but two more
were added in API level 11 (`si_rgbc` and `is_calibrated`). Since API level 11,
it has had five fields, but we may remove some or add more in API level 16."

Like editions of a textbook, API levels are _immutable_. The contents of a
textbook may change over time, but the contents of the 2nd edition of a
textbook will never change once the 2nd edition has been published. This means
that any two Fuchsia releases that know about a given API level will agree on
the set of API elements that make up that API level.

The Fuchsia platform surface is not limited to FIDL - it also comprises
syscalls, C++ libraries and the methods therein, persistent file formats, and
more. Conceptually, these are all versioned by API level, but in practice,
versioning support for FIDL libraries is much farther along than for other
aspects of the Fuchsia platform surface.

[platform-surface]: /docs/concepts/kernel/system.md

### Targeting an API level

End-developers select a single **target API level** when building a component.
If a developer targets API level 11, their component will have access to
exactly the API elements that were available in API level 11. If a method was
added in API level 12, the component will not see it in their generated
bindings. Conversely, if a method from API level 11 was _removed_ in API level
12, the component _will_ continue to be able to use it.

As part of building a component, the tools in the IDK embed the target API
level's associated **ABI revision** in the component's package. This is often
called the component's "ABI revision stamp".

The ABI revision stamp allows us to guarantee that the _runtime_ behavior
observed by the component will match the behavior specified by its target API
level. Currently, the ABI revision stamp is used very coarsely: if the OS
supports that ABI revision's corresponding API level, the component is allowed
to run. Otherwise (for instance, if an external component targets API level 11
but the OS binaries no longer implement methods that were part of API level
11), Component Manager will prevent it from starting up.

### Releases and Phases

Each Fuchsia release supports _multiple_ API levels, and assigns each API level
a **phase**. You can find the API levels in a Fuchsia release by downloading
its IDK and looking in the `version_history.json` file.

For instance, a hypothetical Fuchsia version `20.20240203.2.1` might include:

- API levels 17, 18, and 19 in the **supported** phase. This means IDK version
  `20.20240203.2.1` can build components targeting any of these API levels, and
  Fuchsia version `20.20240203.2.1` can run components that target any of these
  API levels. Most end-developers should target supported API levels most of
  the time.
- API levels 15 and 16 in the **sunset** phase. Fuchsia will still _run_
  components that target sunset API levels, but the IDK will no longer support
  targeting them when building components.

This hypothetical `version_history.json` would also list API levels 14 and
earlier in the **retired** phase. Fuchsia does not build or run components
targeting retired API levels, but they are retained for posterity.

API levels are **published** in the supported phase, shortly before their
corresponding milestone branch cut. The hypothetical `20.20240203.2.1` canary
release above comes from before API level 20 was published, so API level 20 is
conspicuously absent. Shortly before the F20 branch cut, API level 20 will be
published, and subsequent releases (say, `20.20240301.4.1`) will list API level
20 as "supported". In particular, all _milestone_ releases include their
corresponding API level in the supported phase.

When we choose to drop support for an API level, we first move it to the sunset
phase. This creates a back-stop of sorts - existing binaries targeting that API
level continue to run, but no new binaries targeting that API level will be
created (at least - not with new releases of the IDK). Once all existing
binaries targeting a given API level have been phased out, we can retire it.

API levels are not sunset or retired on any particular schedule. Rather, we
sunset an API level once our partners are no longer building against it, and we
retire an API level once our partners no longer want to run binaries that
target it. Someday, as the number of partners grows and we are less closely
attuned to their individual needs, we will need a more formal policy, but this
will do for now.

### `NEXT` and `HEAD` pseudo-API-levels

In addition to the API levels discussed above, there are two special _mutable_
pseudo-API levels: `NEXT` and `HEAD`.

Unlike other API levels, the contents of `NEXT` and `HEAD` can be changed in
arbitrary ways from release to release. API elements can be added, modified,
replaced, deprecated, and/or removed. In principle, `NEXT` in `20.20240203.1.1`
could look completely different from `NEXT` in `20.20240203.2.1`, though in
practice, changes will be incremental.

As described in [RFC-0083][rfc-0083-head] `HEAD` represents the bleeding edge
of development, intended for use by in-tree clients (for example, other
platform components or integration tests), where there is no expectation of
stability. API elements introduced in `HEAD` may not even be functional; for
example, a method may have been added to a protocol at `HEAD`, but the protocol
server might not actually implement that method yet.

`NEXT` represents a "draft" version of the next numbered API level. Shortly
before each milestone branch cut, we publish the contents of `NEXT` as the new
API level. Mechanically, we do this by creating a changelist replacing
instances of `NEXT` in FIDL annotations and C preprocessor macros with the
specific number of the next milestone branch. API elements in `NEXT` should be
functional, and are _unlikey_ to change before the next API level is published,
though changes are still possible.

In the language of RFC-0083, `HEAD` is newer than `NEXT`, meaning that all API
changes in `NEXT` are also included in `HEAD`.

End-developers may target `HEAD` or `NEXT`, but doing so voids the
[API](#api-compatibility-guarantee) and [ABI](#abi-compatibility-guarantee)
compatibility guarantees. See the sections describing those guarantees for more
about the implications of targeting `NEXT` or `HEAD` from out-of-tree.

[rfc-0083-head]: /docs/contribute/governance/rfcs/0083_fidl_versioning.md#purpose_of_head

## Policy changes

This section lists the specific policy changes proposed by this RFC.

### Model changes

This RFC makes the following changes to the platform versioning model:

- **Numbered API levels are now immutable.** This property is already true in
  practice for all API levels except the special "in-development" API level.
  This RFC effectively deprecates the "in-development" API level, in favor of
  `NEXT`.
- **Introduce API level _phases_**. "Supported" and "retired" already exist in
  practice, though the latter is currently called "unsupported". This RFC
  proposes renaming it, and adding "sunset".
- **Introduce `NEXT`.** This is another special API level, like `HEAD`. `HEAD`
  is "newer than" `NEXT`. This replaces the current practice of designating the
  newest API level the "in-development" API level.
- **Allow backwards-incompatible changes to `NEXT`**. The current policy,
  enforced by tooling but not established in any RFC, is that only
  backwards-compatible changes may be made to the in-development API level.
  This RFC removes that restriction, and will not apply it to `NEXT`. [See
  below for more discussion.](#backwards-compatible-in-development)
- **Couple API levels to milestones**. API level `N` will be created shortly
  before the F`N` milestone branch cut, based on the contents of `NEXT`. This
  has been true in practice, but has not been ratified in any RFC.
- **API levels move through phases as quickly as our partners allow.** We will
  sunset a supported API level as soon as none of our partners need to build
  binaries targeting it, and no sooner. We will retire a sunset API level as
  soon as none of our partners want to run any binaries targeting it on new
  Fuchsia releases, and no sooner.

### API compatibility guarantee

Fuchsia now makes the following API (that is, build-time) compatibility
guarantee for numbered API levels (subject to [caveats below](#caveats)):

> If an end-developer can successfully build their component targeting a
> numbered API level `N` using _some_ release of the IDK, they will be able to
> successfully build that same source code using _any_ release of the IDK with
> `N` in the supported phase.

In other words, an end-developer can fearlessly upgrade or roll back their IDK
without breaking their build, until their chosen target API level is sunset. If
updating to a different IDK without changing their target API level causes
their build to break (for example, if an API element they were using no longer
exists), that will be treated as a Fuchsia platform bug.

On the other hand, there is **no** API compatibility guarantee for the mutable
pseudo-API levels `NEXT` and `HEAD`. When targeting `NEXT` or `HEAD`, an
end-developer may find that updating their IDK causes their code to no longer
compile.

Practically speaking, our current partners work in close coordination with the
Fuchsia team, and we will avoid breaking the builds of end-developers that
target `NEXT` on a "best effort" basis. We will make little-to-no effort to
avoid breaking the builds of end-developers targeting `HEAD`.

### ABI compatibility guarantee

Fuchsia now makes the following ABI (that is, run-time) compatibility guarantee
for numbered API levels (subject to [caveats below](#caveats)):

> A component built targeting a numbered API level `N` will successfully run on
> _any_ release of Fuchsia with `N` in the supported or sunset phase, and will
> not see any [objectionable behavioral changes](#objectionable-changes)
> between releases.

In other words, an end-developer does not need to change or recompile their
code in order to run on newer versions of Fuchsia, up until the point when
their chosen target API level is retired. If the platform behaves differently
in a way that interferes with their component's functionality on a different
version of Fuchsia, that will be treated as a Fuchsia platform bug.

On the other hand, there is **no** ABI compatibility guarantee for components
that target the mutable pseudo-API levels `NEXT` or `HEAD`. A component built
targeting `NEXT` or `HEAD` will be permitted to run if and only if the version
of Fuchsia on which it is running _exactly_ matches the version of the IDK that
built it. For example, a component targeting `NEXT` built with IDK version
`16.20231103.1.1` will run on Fuchsia version `16.20231103.1.1`, but _will not_
([by default](#abi-revision-checks)) run on a device with Fuchsia version
`16.20231103.`**`2`**`.1`.

In most circumstances, ensuring the version of the IDK that builds a component
exactly matches the version of the OS on which it runs is not feasible.
Noteworthy exceptions are integration testing and local experimentation.
Out-of-tree repositories generally _do_ have control over which version of the
IDK they use for compilation and which version of the OS they use for testing.
If they want to be able to test functionality in `NEXT` or `HEAD`, they should
keep the two in sync.

### Unsupported configurations are forbidden {#abi-revision-checks}

By default, `component_manager` refuses to launch components whose ABI revision
stamp indicates that they are not covered by the ABI compatibility guarantee.
Specifically:

- A given release of Fuchsia will only run a component that targets API level
  `N` if `N` is either supported or sunset as of that release.
- A given release of Fuchsia will only run a component that targets `NEXT` or
  `HEAD` if it was built using the IDK from the exact same release.

Likewise, product assembly will refuse to assemble products with components
that are not compatible with the platform.

Product owners will have the ability to selectively disable these checks. For
instance, they will be able to allowlist an individual component to target
`NEXT` or `HEAD`, even if the IDK that built it came from a different Fuchsia
release. Product owners disable these checks at their own risk.

#### Implementation

Implementing these checks will require some changes to ABI revision stamps. A
full design of these changes is beyond the scope of this RFC, but in brief:

- As is the case today, each API level will be assigned a unique ABI revision
  when it is published. Components targeting a numbered API level will be
  stamped with that API level's corresponding ABI revision.
- Additionally, each _release_ will be assigned a unique ABI revision.
  Components targeting `HEAD` or `NEXT` will be stamped with this per-release
  ABI revision.

The OS binaries in a given release will include a list of the ABI revisions for
all API levels in the supported or sunset phases, as well as the ABI revision
for that release. By default, only components stamped with one of these ABI
revisions will be allowed to run.

#### Future extensions

Note that the "one ABI revision per API level, plus one more for `NEXT`/`HEAD`"
arrangement is not necessarily the last word on this design. ABI revisions are
how we encode our ABI compatibility guarantees in a machine-readable way, so if
our guarantees become more nuanced, so too must our allocation of ABI
revisions.

For example, say we designate part of our API surface as having "Long Term
Support (LTS)", and continue supporting the LTS APIs within an API level for
longer than other APIs. A hypothetical F30 release may support the LTS APIs
from API level 25, but not the non-LTS APIs. That release would be able to run
components that targeted API level 25 using only LTS APIs, but could not run
components targeting API level 25 that used non-LTS APIs. Thus, these two kinds
of components would need to be stamped with different ABI revisions, which we
could accomplish by giving each (API Level, LTS status) pair its own ABI
revision.

We have ideas along these lines, and if we choose to pursue them, they will be
described in a subsequent RFC.

### Platform components support all supported API levels simultaneously

Since we currently have no mechanism by which platform components can alter
their behavior based on the ABI revision stamp of the component on the other
side of the connection, supporting multiple API levels means supporting
multiple API levels _simultaneously_. The behavior of the platform component
must meet the specifications of each API level in the supported or sunset
phase.

More precisely, a platform component _must not_ assume anything about the API
level targeted by an external component, except that it is targeting one of the
supported or sunset API levels.

As a simple example, if a method `Foo` was removed from a FIDL protocol in API
level 15 (that is, `Foo` was annotated with `@available(added=A, removed=15)`),
but API level 14 is still supported, platform components that implement the
protocol server in question _must_ continue to implement `Foo` until all API
levels that include it have been retired.

Conversely, if that FIDL protocol server was implemented by an out-of-tree
driver, a platform component communicating with that driver must work
regardless of whether the driver implements `Foo` or not.

This strategy constrains not only the _implementation_ of platform components,
but also kinds of modifications we can make to the platform surface.
Theoretically, APIs may change arbitrarily from API level to API level, but
practically speaking, some changes (for instance, reusing a FIDL protocol
method ordinal) leave us unable to correctly communicate with external
components targeting different API levels without further information.

We may develop different strategies for supporting multiple API levels, but
that's an area for future work.

## Caveats

Both the API and ABI compatibility guarantees are subject to a few caveats.

### The meaning of "guarantee"

"Guaranteeing" compatibility does not mean that Fuchsia will never break our
compatibility promises - to err is human. It means that _when_ we break
compatibility according to the terms above, we consider that a bug. We will
take responsibility for remedying the situation internally, without requiring
any action from the end-developer, beyond reporting the bug.

### Voiding the guarantee

The API and ABI guarantees do not apply if end-developers are "doing something
weird." This includes, but is not limited to:

- modifying files in their copy of the IDK,
- accessing members in internal namespaces,
- manually synthesizing data that is intended to be built by libraries in the
  IDK,
- linking shared libraries built by different IDKs into a single binary,
- etc.

As a notable counter-example, depending on buggy behavior in Fuchsia _does not_
count as "doing something weird." In some unfortunate circumstances, _fixing_ a
bug may count as an "objectionable behavioral change." See [Objectionable
changes](#objectionable-changes) for more discussion.

A full accounting of prohibited behaviors is out of scope for this RFC. To
paraphrase United States Supreme Court Justice Potter Stewart in _Jacobellis v.
Ohio_, "I shall not today attempt further to define \['doing something
weird'\], and perhaps I could never succeed in intelligibly doing so. But I
know it when I see it."

### Objectionable Changes {#objectionable-changes}

The ABI compatibility guarantee promises that components will not see
"objectionable behavioral changes" between Fuchsia releases that support the
component's target API level. The definition of "objectionable" is clearly
subjective, and changes that are desirable to some may be objectionable to
others ([relevant xkcd][xkcd-1172]). Ultimately, reports of objectionable
behavior changes will need to be addressed on a case-by-case basis.

[xkcd-1172]: https://xkcd.com/1172/

### Security and privacy

If certain parts of the Fuchsia platform surface are found, in retrospect, to
have significant security or privacy issues, we may selectively drop support
for the offending functionality, as necessary to meet our security and privacy
goals.

## Performance

Maintaining backwards compatibility with older API levels has a performance
cost. If nothing else, backwards-compatible platform binaries will have to be
larger, as more logic will be retained. To some extent, this is inevitable -
however, the fact that [API levels are retired atomically](#api-levels-atomic)
potentially exacerbates the problem.

## Backwards Compatibility

The whole of this RFC concerns compatibility, both backward and forward.

## Testing

Testing to ensure API and ABI compatibility is another large topic.

The Compatibility Tests for Fuchsia (CTF) effort aims to provide test coverage
to confirm we meet our ABI compatibility guarantee. Coverage is currently low,
but that is being actively worked on.

Currently, we are unable to write tests to verify our API compatibility
guarantee; all of our in-tree tests are built against the `LEGACY` API level,
so we cannot replicate the experience of an out-of-tree customer targeting a
numbered API level. This will need to be addressed, but doing so should not
block acceptance of this RFC.

## Documentation

Upon ratification of this RFC, its text will be reused as the basis for
multiple documents targeted at Fuchsia platform developers:

- The compatibility policies themselves.
- A guide to API levels and compatibility.
- The beginnings of a document outlining usage guidelines that [void our
  compatibility guarantees](#voiding-the-guarantee) (that is, "doing something
  weird"), perhaps modeled after [Abseil's compatibility guidelines
  document][absl-compat].

Documentation will also need to be updated to reflect the renaming of existing
concepts.

[absl-compat]: https://abseil.io/about/compatibility

## Drawbacks, alternatives, and unknowns

### Drawback: API levels are atomic {#api-levels-atomic}

As laid out by [RFC-0002], API levels cover the entire Fuchsia platform
surface. Therefore, changes to an API level's phase (e.g., retiring an API
level) affect the whole API level; we can't retire just a subset of the
functionality. Likewise, an end-developer targeting a given API level targets
that API level for the whole Fuchsia platform surface; they can't, for
instance, target API level 13 for most APIs but API level 16 for one particular
API.

Future extensions to the Platform Versioning model may allow for meaningful
subdivisions, but dependencies between the various libraries in the IDK make
this challenging.

### Alternative: No sunset phase

An earlier draft of this RFC omitted the sunset phase, leaving it as "potential
future work". The sunset phase is a very reasonable extension to the model, and
it was brought up frequently enough in discussions that proactively including
it seems appropriate.

### Alternative: Other possible pseudo-API levels

As described above, `HEAD` has multiple potential use cases: communication
between in-tree components, APIs exposed to integration tests, experimental
APIs, etc. Should these actually be represented by distinct pseudo-API-levels?

This RFC proposes starting with just `NEXT` and `HEAD`, but leaving gaps
between them when assigning numerical values. That way, more pseudo-API-levels
can be added if we find use cases for them.

### Alternative: Backwards-compatible in-development API level {#backwards-compatible-in-development}

Fuchsia's de-facto versioning policies current say that only _backwards
compatible_ changes may be made to the in-development API level. This RFC rolls
back that policy, and as a result, makes no guarantee about compatibility
between an IDK and OS binaries from different releases when the end-developer
is targeting `NEXT`.

If we maintained this policy of backwards compatibility within the
in-development API level (or `NEXT`), we could do better: we could guarantee
that an IDK and OS binaries would be compatible as long as the OS binaries come
from a _newer_ release than the IDK. We relied on this property until
relatively recently, when we moved downstream repositories to target supported
API levels.

This has the benefit that end-developers can safely start depending on new
Fuchsia functionality as soon as it is added to the in-development API level -
they don't need to wait for that API level to get published.

However, it introduces significant complexity to the system:

- Fuchsia platform developers must maintain backwards compatibility not only
  with previous API levels (the API signatures of which are encoded in FIDL and
  header files themselves), but also with previous versions of the
  in-development API level (which are not). The only way to see these previous
  versions is to examine git history, which in practice does not happen.
- Since [we need to be able to remove functionality](#dont-remove-things) from
  the platform, there needs to be _some_ API level that supports
  backwards-incompatible changes. This means Fuchsia platform developers must
  make backwards-incompatible changes to the numbered API level _after_ the
  in-development API level. Thus, there are effectively _two_ in-development
  API levels, and Fuchsia platform developers need to reason about both.
- It makes ABI revisions significantly more complex. To support
  backwards-compatible changes within the in-development API level, we would
  need to retain the [per-release ABI revisions](#abi-revision-checks) (of
  which several are created every day) indefinitely. The OS binaries would need
  to support the ABI revisions for each release during which one of its
  supported API levels was the in-development API level, as well as all the ABI
  revisions for releases from the current milestone so-far. At our current
  release cadence, this would add up to around 168 ABI stamps per API level,
  and as of this writing, a release of Fuchsia would need to support more than
  800 ABI stamps. While the memory costs of such a solution are unlikely to be
  significant, the complexity of maintaining compatibility with hundreds,
  thousands, or tens of thousands of distinct ABIs does not seem worth it at
  this time, especially since none of our customers currently use the
  in-development API level.

This RFC doesn't preclude the possibility of offering additional compatibility
guarantees in the future.

### Alternative: Decouple API levels from milestones

This RFC explicitly couples API levels to milestone numbers. This is not
strictly necessary - notably, [Android][android-api-levels] decouples the two.
The Android project uses this capability to release API changes alongside minor
versions of the OS. For example, API Level 24 corresponds to Android 7.0, and
API level 25 corresponds to Android 7.1.

In my research, I have not found another example of a platform that chose to
follow this path. Other platforms I found describe availability of API elements
in terms of release versions, for example, "Available since Chrome 88" or
"Deprecated in iOS 6.1."

macOS and iOS API levels are coupled to their versioning schemes, but both
allow API changes at minor revisions, unlike Fuchsia. However, this appears to
be largely motivated by a desire to couple their releases' major version with
the calendar year, while still allowing API updates to ship multiple times a
year.

Fuchsia's milestones are more closely modeled after Chrome's, incrementing the
major version frequently, and leaving the minor version unused. Should we wish
to stabilize API levels more frequently, we can do so by incrementing Fuchsia's
milestone number more frequently.

[android-api-levels]: https://developer.android.com/tools/releases/platforms

### Alternative: Numbered "in-development" API level

An earlier draft of this RFC stayed closer to the Platform Versioning model
that is currently implemented. In that model, `NEXT` does not exist, and
instead, the largest numbered API level is mutable and called the
"in-development" API level.

Mechanically, there are only a few small differences between the models:

1. Currently, Fuchsia platform developers chose a particular API level at which
   they want their feature to launch. After this RFC is implemented, they will
   make production-bound API changes to `NEXT`, which will automatically be
   published in the next numbered API level.
2. Currently, end-developers targeting the in-development API level
   automatically transition to targeting the supported version of that API
   level once it is published. After this RFC is implemented, they will
   continue targeting `NEXT`.

Neither of these differences seems overwhelmingly positive or negative.

The main benefit of `NEXT` over the in-development API level is ease of
communication. Earlier drafts of this RFC resorted to verbal gymnastics to
express the compatibility guarantees: "A component built targeting supported
API level `N` will successfully run on any release of Fuchsia that supports API
level `N` in the supported phase." In the current proposal, all parties will
agree about the contents of a given API level, with no qualifiers necessary.

### Alternative: Don't remove things {#dont-remove-things}

We could make Platform Versioning simpler by disallowing backwards-incompatible
changes altogether. Each API level would support all functionality from
previous ones, and the "platform supports all API levels" property would come
"for free" by virtue of supporting the latest API level.

Many platforms including Windows and Android have largely taken this approach,
but as a result they are unable to clean up cruft from older revisions of their
API surface.

Though Fuchsia platform engineers will continue to need to support old API
elements as long as they appear in a supported or sunset API level,
end-developers will only need to worry about the contents of their chosen
target API level, and won't need to see the cruft.

Furthermore, though "never removing things" may make versioning _simpler_, it
by no means solves the problem entirely. End-developers still need to reason
about which API level to target, based on the oldest Fuchsia platform version
in their intended install base.
