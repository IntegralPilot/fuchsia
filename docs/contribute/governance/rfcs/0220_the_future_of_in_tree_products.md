<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0220" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View
the #} {# fully rendered RFCs at
https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

A proposal to simplify the set of product configurations in the open source
[//products directory][products-dir].

## Motivation

[`//products`][products-dir] currently contains ten product definitions. Fuchsia
has evolved significantly since those product definitions were created, and the
users of those products have become fragmented. Some products have only a couple
of users or use cases, or have idiosyncratic features like UI stacks that only
exist in that product. Making changes to cross-cutting features like product
assembly, the build system, or graphics or networking stacks requires
considering each individual product and their differences. This costs developer
time and effort, and slows down the platform's evolution.

The `//products` directory also has no clear roadmap or statement of purpose. It
is unclear from the `README` why the products in that directory exist, who
they're for, or who maintains them. An ad hoc group of volunteers cares for and
feeds the product definitions, with no clear mandate.

Rather than a larger number of products in the platform source tree which have
specific use cases, Fuchsia should prefer a smaller number of tightly scoped
product definitions that provide reference implementations and examples for how
to create a product out of tree (OOT),

It is time to set a roadmap for the `//products` directory and significantly
reduce the user base fragmentation of Fuchsia's product definitions.

## Stakeholders

* Infrastructure
* Fuchsia Engineering Council
* Product management
* Users of products we propose to modify or remove
  * Workstation
  * Terminal
  * Core
* Software Assembly
* Build

_Facilitator:_ `rlb@google.com`

_Reviewers:_

In alphabetical order

* `aaronwood@google.com`
* `abarth@google.com`
* `amathes@google.com`
* `ddorwin@google.com`
* `dworsham@google.com`
* `fmeawad@google.com`
* `hjfreyer@google.com`
* `jasoncampbell@google.com`
* `neelsa@google.com`
* `nicoh@google.com`

_Consulted:_

List people who should review the RFC, but whose approval is not required.

* Software Assembly team
* Build team
* Performance team
* Chromium team
* Driver development team
* Bringup team


_Socialization:_

This RFC was socialized internally in document form with much of the
organization, including the teams listed as Consulted and the Fuchsia
Engineering Council.

## Glossary

### System

A **system** is a set of images ([ZBI][glossary.zbi], [vbmeta][glossary.vbmeta],
and optionally an [FVM][glossary.fvm]) destined for a single boot slot on a
target. This represents a bootable Fuchsia image, though without everything we'd
want to flash to a target in production (like recovery images).

See [RFC-0115][rfc-0115] for more details on boot slots and flashable images.

### Product

From our [existing documentation][product-definition]: "A product defines the
software configuration that a build will produce. Most critically, a product
typically defines the kinds of user experiences that are provided for, such as
what kind of graphical shell the user might observe, whether or not multimedia
support is included, and so on."

In the output of our [product assembly process][product-assembly-process], a
**product** defines the combination of one or more systems (in slots A, B,
and/or R) that are intended for placement on a target. For example, a product
image might contain a kernel image, FVM, vbmeta, a recovery image, a recovery
vbmeta, and a bootloader image.

### Board

From our [existing documentation][board-definition]: "A **board** defines the
architecture that the build produces for, and key features of the device upon
which the build is intended to run. This configuration affects what drivers are
included, and may also influence device-specific kernel parameters."

Fuchsia build parameters are specified by a (product, board) tuple. This set of
parameters completely describes the work the build system must complete to
produce a flashable product image.

## Goals

* Minimize the number of product definitions the Fuchsia team has to support
  over time, to minimize load on both developers and infrastructure

* Minimize the amount of migration work for existing users of the current
  product definitions - moving off `core` and `terminal` should be as easy as
  possible for current users of those products

* Remove product definitions that no longer represent the intended usage of the
  Fuchsia platform, or use deprecated configurations

* Simplify the product definitions that stay so they are easier to maintain

* For product definitions that remain (except `bringup`), implement [build
  types][rfc-0115] so production products can refer to the appropriate build
  type when implementing their own versions

* Define how the new product definitions should be used, when they should be
  added to, and under what conditions we'd add an entirely new product
  definition

* Don't fundamentally change how products are defined, or how boards are
  defined, if possible. Keep scope achievable over the medium term


## Requirements

* We must support kernel and new board development without the need for custom
  product definitions

* All products must be automatically tested in infrastructure

* All products in `//products` should be distributed as SDK companion images,
  and work equally well in and out of tree

* Avoid the need for tests to ever add things to the platform to run on
  products. Tests should be able to bring their dependencies in their own
  package as [subpackages]. For example, `//bundles/buildbot:foo` should only
  contain tests, and should not add anything to the product or platform
  definitions.

## Non-goals

* Restructure or redefine the interaction of boards, build types, and products.
  This RFC should focus as much as possible on the contents of `//products` and
  their purposes.

* Change development workflows beyond swapping product definitions. This RFC is
  only concerned with the contents of product definitions.

## Design

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD",
"SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be
interpreted as described in [IETF RFC
2119](https://tools.ietf.org/html/rfc2119).

We have ten product definitions in [`//products`][products-dir] today. We'll
remove eight of these product definitions over time, keep two (`minimal`,
`bringup`), and add one more: `workbench`. We'll only create `_eng` [build
types][rfc-0115] of `minimal` and `workbench` for now, but may add `_user` and
`_userdebug` build types in future if there is a need.

Many of the current definitions exist to support testing flows, and contain
their particular packages and configuration because tests directly rely on
services included in the system image, rather than bringing their dependencies
with them in their own package.

We intend that _most_ Fuchsia tests should run on `minimal_eng` in the medium
term, because most Fuchsia tests should be hermetically packaged with all their
dependencies.

If a particular test needs to run while relying on services provided directly by
the system (not versions running out of its own package), then we should avoid
creating a generic in-tree product for testing it. That test should run on
the product it's meant to test functionality for, or it should define its own
private out-of-tree product definition to run on. We will no longer create and
maintain generic "testing products" for tests of all possible functionality.
Instead of using products only meant for testing, we should prefer to add
testing support to products which are more generally useful for customers or
developers.

This means that not all functionality in fuchsia.git will exist in an in-tree
product's base configuration by default. We should prefer to create integration
and unit tests which can run hermetically on the products we have, rather than
add all functionality to a platform-supported product and support that forever.


### Product definitions to preserve


#### bringup.gni

Used mostly by kernel developers today. `bringup` is used to exercise the kernel
and core drivers and to do development on new boards.

We intend to leave `bringup` as it is in terms of its purpose and contents, but
move it to use product assembly. We no longer intend `bringup` to be the "base
class" for all defined products, as bringup has certain features that are not
applicable for all products.


### Product definitions to add or repurpose

The remaining product definitions in `//products` will be co-owned by the
Fuchsia Architecture and Software Assembly teams. Day to day maintenance will be
done by Software Assembly.

#### minimal - keep, make really minimal

[Intended][minimal-definition] to be "the smallest thing we'd still call
Fuchsia."  Definitionally, the smallest thing we'd still call Fuchsia is a
system which can:

* boot into userspace
* run Component Manager and components
* update itself using our [over-the-air update system][ota-updates]
  * this implies that storage and networking are working, with drivers provided
    by the board

Note: by "smallest," we're referring to the smallest amount of functionality the
system requires in order to accomplish the above tasks. For instance, none of
the above tasks require graphics support, so a system which had graphics would
be considered "larger" than `minimal`. "Smallest" does _not_ necessarily mean "a
system which uses the least memory or CPU." `minimal` might have the lowest
resource usage of any product, but that would be by chance rather than design.

The main use of this product for the Fuchsia project will be to run
hermetically-packaged tests which contain all their dependencies. It can also
serve as a basis for system tests which are not hermetic but only depend on the
small set of Fuchsia services it contains.

We will define `minimal` as an `_eng` build type and for now not create `_user`
or `_userdebug` versions of `minimal`. All product definitions in product
assembly must be one of `_eng`, `_userdebug`, or `_user`. All references to
`minimal_eng` and `minimal` in the rest of this document are therefore
equivalent.

`minimal` will include support for:

*   Components
*   Packages, and the ability to OTA the system
*   Flashing the system via fastboot
*   Driver framework and a truly tiny number of drivers (ideally zero; we should
    prefer to add drivers in the board definition rather than the product
    definition unless those drivers are required in _all_ Fuchsia products)
*   Logging
*   Networking (perhaps including USB CDC ethernet if the board has no other
    networking capability)
*   Storage
*   Session support, though without a session specified
    *   Sessions will be added at runtime using `ffx session launch` on `eng`
        builds.
*   Size limits support for the above features

`minimal_eng` will be the recommended configuration to run tests against, and
will non-exhaustively include by virtue of being an `_eng` configuration:

*   SSH support
*   Remote Control Service support
*   Test manager

We do not intend to include the following:

*   Paving support through zedboot (though the paver _service_ needs to work
    on a running Fuchsia image in order to enable OTAs)
*   Graphical support
*   Audio support
*   Input support
*   Virtualization support
*   WLAN support
*   Bluetooth support

**Note:** even though `minimal` won't include support for these features in
base, **they can still be tested on `minimal_eng` builds in
hermetically-packaged tests,** because all `_eng` product builds contain test
support. For instance, a graphics test could subpackage the Fuchsia UI subsystem
and hardware fakes, as provided by [test\_ui\_stack][test-ui-stack], and run on
`minimal_eng`, even though no graphics will be shown on a screen. We will
encourage all tests to run on `minimal_eng` instead of `workbench` if they are
able to do so, as `minimal_eng` will have lower build time and more
debuggability by virtue of being smaller.

##### When should a platform component get added to `minimal`?

A platform developer should add a component to an [Assembly Input Bundle
included][aib-docs] in `minimal` when it supports the core goal of the `minimal`
product. For example, a new default filesystem for Fuchsia should get added to
`minimal`, as a filesystem is required for OTA updates and `minimal` should use
the default Fuchsia filesystem. A new driver framework should get added to
`minimal` if it is intended to be in all Fuchsia products. A new graphics driver
or control system should **not** get added to `minimal`, as graphics are not
required in all Fuchsia systems to boot or update the system.

We have two pressure tests to firmly determine whether a given component should
be included in `minimal_eng`:

1. Should that component be in _all_ fuchsia products at the `_eng` build type?
1. Is `minimal` unusable without it?

If a platform component shouldn't go in minimal, it is likely still a fit for
`workbench`.

##### Drivers in `minimal`

We should include support for a driver stack in the platform configuration for
`minimal` if it is critical to support an OTA or running component manager on
all boards. This means that even if the board running `minimal` provides support
for say, Bluetooth, we shouldn't include that driver stack in the resulting
configuration because the product does not need it.

This implies a design in which the set of drivers included in the final image is
the [_intersection_][intersection] of features requested by the product and the
board's hardware support. This intersection feature is in development with the
Software Assembly team.

#### workbench

`workbench` will be a product configuration for local development, running
larger tests which cannot or should not be packaged hermetically, and exercising
larger parts of the Fuchsia platform than `minimal` supports. It is intended to
be like a literal workbench in that it supports development tools and allows a
developer to poke at the system and make changes.  It is not intended to be a
product that ships to users, or be a basis for those products.

We intend that `workbench` will include graphics, audio, and input support
(touch, mouse, and keyboard) in addition to the features included by `minimal`.
This will enable full system validation tests which simulate a graphical product
(with the exception of hardware accelerated video decoders and DRM/protected
memory).

We will also include WLAN and bluetooth support in `workbench` builds if they
are supported by the board configured for the build.

This product will serve most uses of the current `workstation` and `terminal`
products, but at a much lower maintenance cost. For instance, many tests that
currently run on `core` will need to run on `workbench`, so `workbench` will be
able to build with the equivalent of `--with //bundles/buildbot:core` to create
a package repository of available tests.

`workbench` will have a default session adapted from `tiles-session`, but will
not launch any elements by default. This will enable interactive testing and
debugging of different graphical elements, like graphics demos or terminals.
Software or tests which need the session to not launch by default will be able
to use runtime configuration to disable the launch.

`workbench` will not contain any runtimes like Flutter or Chrome as base
packages, but they will be able to be loaded on-demand from a package server for
development.

`workbench` will use the same platform definition for assembly as `minimal`, but
add to it. It will not inherit the entire product configuration of `minimal`.

We will define `workbench` as an `_eng` build type and for now not create
`_user` or `_userdebug` versions of `workbench`. All product definitions in
product assembly must be one of `_eng`, `_userdebug`, or `_user`. All references
to `workbench_eng` and `workbench` in the rest of this document are therefore
equivalent.


##### When should a platform component get added to `workbench`?

We should add components to `workbench` when it is expected that the component
will be used in all Fuchsia products with graphics and audio support, or that
most in-tree Fuchsia developers would benefit from its inclusion in testing
flows or day to day development.

For example:

* All of the things we add to `minimal` should also be added to `workbench`
* We should add new graphics, audio, and general Fuchsia development tools and
  components in their default configurations
* We should **not** add components which are only intended for a small subset of
  Fuchsia products. For instance, we should not add a camera implementation to
  `workbench`, since only a subset of Fuchsia products will have a
  camera.


##### Product-provided implementations of platform protocols (e.g. fuchsia.web)

Some FIDL protocols like `fuchsia.web` are defined by the platform, with
implementations that are provided from outside the platform. Since those
implementations can change and the platform doesn't generally contain a
component implementing those protocols, we need to provide a place in the
component topology for the product to slot in a component implementing those
protocols.

In particular, we should leave a place in the `workbench` component topology for
an implementation of `fuchsia.web` without providing an implementation in the
in-tree product definition. Product owners or developers who wish to include an
implementation of `fuchsia.web` can either use `--with-base` during a build,
place an implementation in the Fuchsia package server to be loaded at runtime,
or we could add support for specifying product-provided implementations directly
in the product assembly configuration.


### Product definitions to remove

We'll migrate users away from each of these product definitions over time, and
then delete the definitions.


#### core.gni

As of this RFC, `core` and all of its various sub-definitions are *deprecated*.

Used extensively in Fuchsia's infrastructure and by Fuchsia developers at their
desks. This is far larger than "the smallest thing we'd call Fuchsia," and
includes things like [Bluetooth tooling][core-bluetooth], [WLAN
tooling][core-wlan], a [driver playground][core-driver-playground],
[audio][core-audio], the [Starnix manager][core-starnix-manager], a [test
manager][core-test-manager], [fuzzing support][core-fuzzing], [virtualization
support][core-virt], [SSH support][core-ssh], a [utility for managing short-term
mutable storage][core-storage], and many other things that not all products will
require.

Our opinion is that `core` is a useful product for certain cases, but contains
too many opinionated choices about what a product should contain to be the basis
of _all_ Fuchsia products.

We plan to shift infrastructure and developers off of `core` and to `minimal`
and `workbench` over time, and eventually delete `core`. We'll make this
possible with technology like [subpackages], hermetic integration tests, and
making it easy to add software at runtime using Software Delivery tools.

The `core.x64` builder runs well over 1000 tests in Fuchsia infrastructure, so
deleting this product will be a significant task without technology to ease the
transition.

All tests that we currently have should continue to run on either `minimal_eng`
or `workbench_eng`, and we must continue to support the `//:tests` idiom in
which developers can put their tests in a top-level GN label and they will be
sure the tests will run on infrastructure _somewhere_.

Guidance for where a test should live follows:

1. if a test is hermetic (that is, can't resolve packages or services outside of
   its own package) or is explicitly tagged by a test spec as compatible with
   `minimal_eng`, then we should prefer running it on the `minimal_eng` builder
1. if a test is non-hermetic or we don't have a hermeticity analysis for it, it
   should run on `workbench_eng`, which is the closest analogue to today's
   `core` image

We can likely use programmatic analysis of the `//:tests` target to determine
hermeticity and thus which image runs a given test.


#### core\_size\_limits.gni

Used by [infrastructure][core-size-limits-infra] to perform size checks against
platform code. This in turn allows debugging of size creep by core platform
components before it rolls into a shipping product, and allows a public view of
size check results.

We intend to migrate these checks to `minimal_eng` over time, which should
have built-in size limits. `core_size_limits` exists because there are so many
ways to modify the `core` image at build time (i.e. with GN arguments) that size
limits on the main core image were impractical. Part of the point of `minimal`
is to be unmodifiable at build time, and to come in only one configuration, so
size limits should be an integral part of the `minimal` configuration.

_Note_: We'd prefer that size checking not rely on a specific product
definition. For instance, it would be nice if we could track the size of groups
of platform components over time without relying on a particular product
configuration. However, this capability does not currently exist, and so we're
proposing doing size checks against `minimal_eng` for now. As a result, the
reported overall image size will be affected by non-production packages such as
developer tools, but sizes of production packages will also be available.


#### core\_with\_netstack3.gni

Used as a convenience product for `netstack3` developers and for automated
testing in infrastructure. We should include `netstack2` in `workbench` for the
purposes of testing while `netstack3` is finished, and provide a build-time
switch to enable `netstack3` either via a new product definition like
`core_with_netstack3` that modifies `workbench` or by an argument to the GN or
Bazel build defined in an infrastructure recipe.


#### terminal.gni

As of this RFC, `terminal` is *deprecated*.

Described in the documentation as "a system with a simple graphical user
interface with a command-line terminal." Currently used by a variety of OOT
customers to run tests against a product with a graphics system. We intend to
migrate these users to the `minimal` product (or `workbench` if they really
cannot shed their system dependencies), and then delete the `terminal` product.

The terminal builder in CI [runs][terminal-ci] some benchmarks and [several
significant tests][terminal-uses]. These could likely be migrated to a `core`
builder in the short term, or `minimal` in the long term.

More importantly, we'd need to figure out how to stop distributing `terminal`
images in the SDK. The Chromium and Flutter infrastructures make significant use
of the `terminal` image as a test host. The implementation section of this RFC
describes a plan to migrate the Chromium tests to `workbench`.


#### workstation.gni

As of this RFC, workstation is *deprecated*.

We'd like to replace most uses of workstation with `workbench`.

There are several uses of `workstation` for which we'll need to find
alternatives:

*   A test configuration for system validation tests with a graphics stack
    *   We'll need to migrate these to `workbench`.
*   Virtualization
    *   We don't currently have a roadmap for virtualization (it's being worked
        on), but we may want to migrate virtualization testing and development
        to `minimal` or an OOT product repository.
*   Wayland bridge development
    *   We no longer wish to support this. No replacement will be offered.
*   Starnix development
    *   We'll replace the primary development target for first-party Starnix
        development with an internal product in `//vendor/google`, though it
        will still be possible to run starnix programs on `workbench` using a
        package server.


#### workstation\_eng.gni

See the section on `workstation` above. We will delete this configuration.


#### workstation\_eng\_dfv2.gni

This was used as a development target for Driver Framework version 2. The Driver
Framework team is deleting it as they're nearly done with their migration.


#### workstation\_eng\_paused.gni

Used as a basis for [system validation tests][system-validation-tests]. We'll
migrate these tests to `workbench`, though none of them are currently running.

### Development workflow changes


#### Driver development

Driver developers currently do their work mostly on the `core` and `bringup`
products. Some of them modify the board configuration to include their drivers,
some to use `--devs_boot_labels` or `board_driver_package_labels` as an argument
to their `fx set` or `args.gn`, and some use ephemeral driver loading (which is
still experimental).

We are investing in out-of-tree driver development workflows, and the intention
is that driver developers will not need to completely reassemble a product to
test their driver unless that driver is necessary before networking is
available. As of this writing, ephemeral driver loading works for some drivers,
though it is not currently possible to use ephemeral loading for drivers which
need to run before the device's networking is up.

This RFC does not aim to change driver development workflows, but rather to
change the default contents of the product configurations they target. Products
generally should not be pulling in drivers as dependencies, but leaving that to
board configurations.

There are upcoming changes to product assembly and Fuchsia's developer flows
which lead to assembly. We'll leave those flows to subsequent RFCs and documents
by saying: we intend to preserve the ability to include drivers in an assembly
from the command line or via a file like `args.gn`.


## Implementation


### Add new product definitions


#### minimal

*   Ensure that `minimal` has no legacy assembly bundle (this is not a blocker
    but will significantly reduce maintenance cost)
*   Ensure that `minimal` runs well on NUC, VIM3, FEMU, and QEMU


#### workbench

*   Define a method not based on GN inheritance (since we won't be using GN to
    define products, see [RFC-0186][rfc-0186]) to compose `workbench` on top of
    the platform definition for `minimal` to ensure the two products don't
    diverge.
*   Ensure that `workbench` runs well on NUC, VIM3, Google Compute Engine, and
    FEMU.
*   Get system validation tests running and working well
*   Provide support and documentation for developers who wish to add elements to
    `tiles-session` at runtime in order to run things like graphical terminals.


### Migrate users and remove deprecated product definitions


#### First: Improve subpackages support and distribute packages with the SDK

Many of these migrations will rely on support for subpackages being created out
of tree and on our
[workstreams](https://bugs.fuchsia.dev/p/fuchsia/issues/detail?id=119425) to
support fully hermetic integration tests both in and out of tree. We **must**
implement this feature before migrations will be possible in large numbers.

We must also be able to distribute packages out of tree, which was covered in
[RFC-0208][rfc-0208].

The rest of the workstreams can be run in parallel with each other.

#### core and core_size_limits

This product definition will likely be the hardest to move users away from, as
it has the largest number of users both in and out of tree.

Because of previously lax hermeticity requirements for tests, many tests now
take dependencies on certain aspects of the `core` product definition. For
instance, a test might resolve packages it knows to be in the base set in
`core`. In doing the migration away from `core` as a testing product, we should
prevent these instances of [Hyrum's law][hyrums-law] cropping up again. In
particular, we should:

*   Prevent tests resolving packages outside of their own test realm, by giving
    them resolvers with access only to their own test package and its
    subpackages. This exists for tests today that run in the hermetic test realm
    under test manager, though we still need to port old tests to that new
    framework.
*   Utilize subpackages much more broadly across tests - if a test relies on a
    separate package today, it should probably be subpackaged into the test in
    the future.

All hermetically-packaged tests that are also hermetic with respect to services
can be run unmodified against `minimal`. We should eagerly move as many in-tree
tests as possible to run on `minimal`, and establish an allowlist for tests
allowed to run in `core` to prevent more tests taking dependencies on the
configuration of `core`.

Once we have an allowlist for the users of `core`', we'll engage with the owners
of those tests to migrate them to `minimal` or `workbench`, using the best
practices mentioned above.

Out of tree users will be another challenge, since we can't as easily enumerate
them or their tests. We'll work with each petal owner individually to determine
their usage of `core` and whether they can move to `minimal` or `workbench`. If
they can't move without changes to those products, we can make them, or create
out of tree products specifically for those petal's tests, following the [churn
policy][rfc-0214].

#### terminal

`terminal` may be our most complicated removal because it is the most
complicated open source product except for `workstation`, and it has many users.
This RFC does not claim to have a perfectly complete migration plan for
`terminal`, but it does commit the Fuchsia organization to support customers as
they migrate, either by implementing OOT hermetic testing with subpackages, by
adding OOT products for testing, or by adding dependencies to `workbench`
(roughly in that order of priority).

Here's a sketch of an implementation plan:

*   Determine dependents. Known dependents:
    *   Performance tests in the platform tree
    *   Chromium on Fuchsia
    *   Flutter on Fuchsia
    *   The [Antlion builders][antlion-builders]
*   These dependents are likely using `terminal` because they require graphics
    support, so most of them will likely need to migrate to `workbench`.
*   For each dependent, start pulling in `workbench` and running tests against
    it. Run tests in parallel to ensure parity, then turn off the
    `terminal`-based tests once satisfied.
*   As part of these migrations, move tests which can be moved to hermetic tests
    with dependencies subpackaged.


##### Chromium test migration

Chromium's public CI tests `web_engine` using `terminal`. Their tests rely on
lots of features and services that terminal exposes directly from the base
image, such as graphics. In addition, it relies on test/fake components [such
as][chrome-test-realm]
`fuchsia-pkg://fuchsia.com/flatland-scene-manager-test-ui-stack#meta/test-ui-stack.cm`.

To work unaltered, their tests will need an environment which provides those
services and more. We should not block this work on refactoring Chromium tests,
so we need to provide such an environment. We can either move the Chromium tests
to subpackage their dependencies or provide a [package
repository][package-repository-oot] to resolve packages at runtime.


#### `workstation` and sub-products

`workstation` is being deleted from Fuchsia's CI in parallel with this writing.
However, it is still being shipped in the SDK.

*   For each SDK customer of `workstation`, determine whether they need to
    migrate to `minimal`, `workbench` or an entirely different product
    definition (potentially out of tree) for their testing
*   Once all tests are migrated, stop shipping `workstation` images in the SDK

## Performance

No products we're removing are running in production, so no performance impact
is expected.

The terminal images are currently used to run performance tests. We don't expect
significant performance impacts by moving those tests to `workbench`.

## Ergonomics

We don't intend to change development flows or how products are specified with
this RFC. We intend to change the set of available products, which will affect
what developers target when they want to run tests in tree or using the SDK. We
may also help migrate some tests to a different style (as previously mentioned,
hermetically packaged with their dependencies as subpackages) as part of
implementation.

## Backwards Compatibility

Most of these product definition changes are backwards compatible in that we're
presenting the services that most users need from the new products. For
customers of products which are being renamed or changing contents, we'll
provide soft transitions and notification periods. We'll also do much of the
migration work ourselves, inline with the [Fuchsia Churn Policy][churn-policy].


## Security considerations

We don't intend or anticipate significant security changes due to this RFC's
implementation. If anything, our security posture should improve due to
maintaining fewer product configurations.

None of our new products should be given to users or used in production. These
products should implement as secure a configuration of Fuchsia as possible.
`_eng` variants will naturally allow operations like adding package repositories
that current products like `core` and `terminal` already allow by default.


## Privacy considerations

None of our new products should be given to users or used in production, or for
any personal information. We don't believe there are privacy concerns associated
with these new products.

## Testing

Testing the implementation of this RFC is straightforward: all of the tests we
currently have both in and out of tree should still pass. Some may need to be
migrated to rely on different service implementations, or mocks, or fakes.
However, we should achieve the same test coverage we currently have, and we
should not disable test suites to achieve the goals of this RFC.

## Documentation

We'll update the documentation in the `//products` directory which explains the
purpose of each product. We'll add documentation for developers wishing to add
test coverage on which product they should target for their tests, and how to
use them.

We'll remove or modify documentation on fuchsia.dev related to `workstation` and
the other products we're removing as we do those removals. Tutorials or codelabs
which contain removed or renamed products will be updated as part of our
changes.

We will update [RFC-0095][rfc-0095] to note that this RFC supersedes that one,
and we won't be building `workstation` or its successor `workbench` out of tree
at this time.

## Drawbacks, alternatives, and unknowns

### Drawback: temporarily increased infrastructure workload

The transition from `terminal` and `core` to our new product definitions will
mean that we need to have the new product builders running while we transition
off the old ones. This will increase infrastructure load until we can delete the
builders for the old product definitions. This may create churn on the developer
experience as we do the transition.

### Alternative: Do nothing

The main alternative to this RFC is to simply do less. We could remove
`workstation` and keep `core`, `minimal`, `terminal`, and all their variations
unchanged.

This would mean we still need to maintain these product configurations, and work
we do on, e.g. rationalizing the core realm, or migrating products to
bazel-based assembly will need to be replicated across a larger number of
supported products. Having a smaller number of product configurations over time
will be dramatically simpler for the teams which maintain the core aspects of
product configurations, assembly, and build.

Since many of our current customers for these products have less demand given
our reprioritization of workstation, this is an optimal time to simplify our
supported configurations.


### Alternative: Create workbench out-of-tree

We could create `workbench` in an out-of-tree repository (similar to the
proposal in [RFC-0095][rfc-0095]), as many of our tests could be packaged
hermetically and run on `minimal`. However, lots of tests and use cases rely on
graphics or audio, or don't want to pull in another OOT dependency, and we need
something to run in-tree graphics and audio tests on anyway. We'd end up with a
configuration like `workbench` checked into the tree _somewhere_. It's better if
we simply maintain it in the `//products` directory.

## Prior art and references

Linked at relevant points in the document.

[aib-docs]: https://cs.opensource.google/fuchsia/fuchsia/+/main:bundles/assembly/BUILD.gn
[antlion-builders]: https://ci.chromium.org/p/fuchsia/builders/global.ci/workstation_eng_paused.qemu-x64-debug-antlion
[board-definition]: /docs/development/build/software_assembly/build_configuration.md#boards
[chrome-test-realm]: https://source.chromium.org/chromium/chromium/src/+/main:fuchsia_web/common/test/test_realm_support.cc;drc=78920be75f4920a643dcece7ab29742a99377e2c;l=123
[churn-policy]: /docs/contribute/governance/rfcs/0214_fuchsia_churn_policy.md
[core-audio]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=29;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-bluetooth]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=20;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-driver-playground]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=20;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-fuzzing]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=42;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-size-limits-infra]: https://fxbug.dev/42159247
[core-ssh]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=69;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-starnix-manager]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=32;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-storage]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=74;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-test-manager]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=41;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-virt]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=45;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[core-wlan]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/common/core.gni;l=21;drc=c1946eee7aef8fe008361979a4d380e41809ca5e
[glossary.fvm]: /docs/glossary/README.md#fuchsia-volume-manager
[glossary.vbmeta]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/assembly/vbmeta/README.md
[glossary.zbi]: /docs/glossary/README.md#zircon-boot-image
[hyrums-law]: https://www.hyrumslaw.com/
[intersection]: https://en.wikipedia.org/wiki/Intersection
[minimal-definition]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/minimal.gni;l=5;drc=668db2670d6d723f1c48849a13e78a109d6dd1c8
[ota-updates]: /docs/concepts/packages/ota.md
[package-repository-oot]: https://bugs.chromium.org/p/chromium/issues/detail?id=1033794#c20
[product-assembly-process]: /docs/contribute/governance/rfcs/0072_standalone_image_assembly_tool.md
[product-definition]: /docs/development/build/software_assembly/build_configuration.md#products
[products-dir]: https://cs.opensource.google/fuchsia/fuchsia/+/main:products/
[rfc-0095]: /docs/contribute/governance/rfcs/0095_build_and_assemble_workstation_out_of_tree.md
[rfc-0115]: /docs/contribute/governance/rfcs/0115_build_types.md
[rfc-0186]: /docs/contribute/governance/rfcs/0186_bazel_for_fuchsia.md
[rfc-0208]: /docs/contribute/governance/rfcs/0208_distributing_packages_with_the_sdk.md
[rfc-0214]: /docs/contribute/governance/rfcs/0214_fuchsia_churn_policy.md
[subpackages]: /docs/contribute/governance/rfcs/0154_subpackages.md
[system-validation-tests]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/system-validation/ui/BUILD.gn;l=70?q=workstation_eng_paused&ss=fuchsia
[test-ui-stack]: /docs/contribute/governance/rfcs/0180_test_ui_stack.md
[terminal-ci]: https://luci-milo.appspot.com/ui/p/turquoise/builders/global.ci/terminal.x64-release-nuc_in_basic_envs/b8788528956526894065/test-results?sortby=&groupby=
[terminal-uses]: https://docs.google.com/document/d/1KPWwpK1ZWo7hyPYKvw78LkBOy0TNzI9QD0ou-f2YEVY/edit?resourcekey=0-lk6hP2FSwFDv4PMkuvx2pQ
