# API levels

The Fuchsia System Interface is constantly changing, and new
[Fuchsia releases][fuchsia-release-doc] are constantly being created. Rather
than reasoning about each Fuchsia release's compatibility with the Fuchsia
System Interface from each other release, which would be overwhelming, a release
is compatible with a limited number of API levels.

API levels are essentially the versions of the APIs that make up the Fuchsia
platform surface. For example, API level `10` is the 10th edition of the Fuchsia
API surface. API levels are synchronized with milestone releases, so API level
10 can also be thought of as the latest stable numerical version of the
Fuchsia platform surface when `F10` was published.

API levels are immutable. While APIs may be added, removed, or changed over
time, the set of APIs and their semantics at a specific API level will never
change once that API level has been published. This means that any two Fuchsia
releases that know about a given API level agree on the set and behavior of
API elements that make up that API level.

The Fuchsia System Interface is not limited to FIDL - it also comprises
system calls, C++ libraries and the methods therein, persistent file formats,
and more. Conceptually, these are all versioned by API level, but in practice,
versioning support for FIDL libraries is much farther along than for other
aspects of the Fuchsia System Interface.

As a user of the Fuchsia SDK, you select a single target API level when building
a component. If you target API level `11`, your component has access to exactly
the API elements that are available in API level `11`, no matter which version
of the Fuchsia SDK you're using. If a method was added in API level `12`, the
component cannot use it in the generated bindings. If a method available in API
level `11` was removed in API level `12`, the component continues to be able to
use it.

When building a component, the tools in the SDK embed the target API level's
associated ABI revision in the component's package. This is called the
component's _ABI revision stamp_.

The _ABI revision stamp_ allows Fuchsia to ensure that the runtime behavior
observed by the component matches the behavior specified by its target API level.
The ABI revision stamp is used very coarsely: if the Fuchsia platform build
supports that ABI revision's corresponding API level, the component is allowed
to run. Otherwise (for instance, if an external component targets API level `11`
but the OS binaries no longer implement methods that were part of API level
`11`), Component Manager prevents it from starting up.

## Example

You can use API levels to make statements like, the
`fuchsia.lightsensor.LightSensorData` FIDL table had three fields in API level
`10` (`rgbc`, `calculated_lux`, and `correlated_color_temperature`), but two more
were added in API level `11` (`si_rgbc` and `is_calibrated`). Since API level `11`,
it has had five fields, but in the future, such as in API level `16`, fields may
be added or removed.

## Differences for platform developers and users of the Fuchsia SDK

As a user of the Fuchsia SDK, you pick an API level which defines the set of
APIs that you can use and (indirectly) the set of platform releases on which
your Fuchsia component can run on. Your component will only be able to use APIs
that existed at that specific API level, not any API that was added after or
deleted before your chosen API level. If you want access to additional APIs, you
should update your component's target API level to access the new functionality.

On the other hand, the platform build targets a set of API levels based on the
contents of [`version_history.json`](/sdk/version_history.json). Thus,
as a platform developer, you need to make sure all your changes are compatible
with all supported API levels. When you add new functionality or otherwise
change the Fuchsia System Interface, you must do so at the [`HEAD`](#head) or
[`NEXT`](#next) API level.

## Phases {#phases}

A _phase_ indicates the level of support that a Fuchsia release provides for
a given API level. An API level can be in a Supported, Sunset, or a Retired
phase.

Each [Fuchsia release][fuchsia-release-doc] supports multiple API levels, and
assigns each API level to a phase.

API levels can be in one of the following phases, listed in the order each API
level goes through:

* [_Supported_](#supported)
* [_Sunset_](#sunset)
* [_Retired_](#retired)

### Supported

Components built targeting API levels in this phase (with any release) run
on the platform release. In addition, you can use the SDK to build
components targeting API levels in this phase.

For example, if API level 17 is Supported in a given release, that means that
the OS binaries from that release can run components that target API level 17,
and the SDK from that release can build components that target API level 17.

### Sunset

Components built targeting API levels in this phase run on the platform
release. However, you cannot use the SDK to build components targeting
API levels in this phase.

For example, if API level 17 is Sunset in a given release, that means the OS
binaries from that release can run components that target API level 17, but the
SDK from that release cannot build components that target API level 17.

### Retired

Components built targeting API levels in this phase do not run on the platform
release, and the SDK does not support them.

For example, if API level 17 is Retired in a given release, the OS binaries from
that release cannot run components that target API level 17, and the SDK from
that release cannot build components that target API level 17.

### Example

For example, a Fuchsia version `20.20240203.2.1` might include:

* API levels `17`, `18`, and `19` in the **Supported phase**. This means SDK
  version `20.20240203.2.1` can build components targeting any of these API
  levels, and devices running Fuchsia version `20.20240203.2.1` can run
  components that target any of these API levels.
* API levels `15` and `16` in the **Sunset phase**. Fuchsia will still run
  components that target Sunset API levels, but the SDK will no longer support
  targeting them when building components.
* API levels `1`-`14` in the **Retired phase**. Fuchsia does not
  build or run components targeting Retired API levels, but they are retained
  for posterity.

API levels are published in the Supported phase, shortly before their
corresponding milestone branch cut. The hypothetical `20.20240203.2.1` canary
release above comes from before API level `20` was published, so API level `20`
is absent. Shortly before the `F20` branch cut, API level `20` will be
published, and subsequent releases (say, `20.20240301.4.1`) will list API level
`20` as "Supported". In particular, all milestone releases include their
corresponding API level in the Supported phase.

## Special API levels {#special-api-levels}

Note: In most cases, you should not need to worry about special API levels
unless you are a Fuchsia platform developer.

In addition to the stable numerical API levels there are three special API
levels:

* [`NEXT`](#next)
* [`HEAD`](#head)
* [`PLATFORM`](#platform)

Unlike other API levels, the contents of `NEXT` and `HEAD` can be changed in
arbitrary ways from release to release. API elements can be added, modified,
replaced, deprecated, and/or removed. In principle, `NEXT` in `20.20240203.1.1`
could look completely different from NEXT in `20.20240203.2.1`, though in
practice, changes will be incremental.

`HEAD` is newer than `NEXT`, meaning that all API changes in `NEXT` are also
included in `HEAD`.

As a user of the Fuchsia SDK, targeting `HEAD` or `NEXT` voids the
[API][api-compability] and [ABI compatibility guarantees][abi-compability]. As
a user of the Fuchsia SDK, you should not target the `PLATFORM` level directly.

### `NEXT` {#next}

`NEXT` represents a "draft" version of the next numerical API level. When the
release team publishes a new API level, `NEXT` is replaced with the new API
level throughout the codebase. This is done by creating a changelist replacing
instances of `NEXT` in FIDL annotations, C preprocessor macros, and Rust macros
with the specific number of the next milestone branch. API elements in `NEXT`
must be functional, and are unlikely to change before the next API level is
published, though changes are still possible.

### `HEAD` {#head}

`HEAD` represents the bleeding edge of development, intended for use by in-tree
clients (for example, other platform components or integration tests), where
there is no expectation of stability. API elements introduced in `HEAD` may not
even be functional; for example, a method may have been added to a protocol at
`HEAD`, but the protocol server might not actually implement that method yet.

### `PLATFORM` {#platform}

Note: This only applies to platform developers.

`PLATFORM` is a special meta API level that is used while building the Fuchsia
platform and is not intended to be targeted by an SDK user.

`PLATFORM` represents a complete set of APIs that the current version of
the Fuchsia platform must support, such as:

* All currently supported stable numerical API levels (see
  [Supported](#supported) and [Sunset](#sunset)).
* The `NEXT` API level.
* The `HEAD` API level.

The main purpose of `PLATFORM` is to ensure that the platform can run components
built against any of its supported stable API levels, as well as those targeting
the `NEXT and `HEAD` API levels. `PLATFORM` includes APIs from all supported
stable numerical levels, so it may contain APIs that have already been removed
from `NEXT` or `HEAD`.

[abi-compability]: /docs/concepts/versioning/compatibility.md#abi-compatibility-guarantee
[api-compability]: /docs/concepts/versioning/compatibility.md#api-compatibility-guarantee
[fuchsia-release-doc]: /docs/concepts/versioning/release.md