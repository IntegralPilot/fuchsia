<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0217" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

Improve the developer experience when running tests or restarting modified
ephemeral components by improving [package garbage collection][docs_gc] (GC).

## Motivation

Common developer workflows, such as:

* [Consecutively][bug_consecutive_test] running multiple tests
* [Repeatedly][bug_repeated_test] running and editing the same test
* Repeatedly running and editing the same ephemeral component

are frequently interrupted by package resolution errors due to insufficient
storage space. This breaks developer concentration, lowers confidence in the
platform, and requires developers to manually trigger GC, possibly after
rebooting the device.

These interruptions and workarounds are necessary because the current GC
implementation:

* Protects some packages that should not be protected (requiring a reboot to
  remove the protection)
* Does not protect some packages that should be protected (making automatically
  triggering GC dangerous, which results in the current stance of avoiding
  automatic GC triggers / preferring manual GC triggers)

The goal is to improve GC so that these workflows can be made to just work
without developers needing to think about GC or storage space at all.

## Stakeholders

_Facilitator:_

- hjfreyer@google.com

_Reviewers:_

- wittrock@google.com (Software Delivery)
- geb@google.com (Component Framework)
- crjohns@google.com (Testing)

_Consulted:_

 - senj@google.com, etrylezaar@google.com, jamesr@google.com

_Socialization:_

An early draft of this RFC was shared with members of the Software Development
and Component Framework teams.

## Requirements {#requirements}

GC as a whole must:

1. Maintain the storage usage and forward progress guarantees of the
   [OTA process][docs_ota] with the modifications from [RFC 170][rfc_0170].
  * In short, allow system-updater to GC intermediate packages it resolves
    during OTA (the update package and the packages containing the incoming
    binary images) without GC'ing the packages required by the next system
    version.
2. Allow running multiple tests (each of which individually fits on the device
   and that all come from different packages) consecutively without running out
   of space.
3. Not remove packages that are being consumed by components, which includes
   packages that contain running components.

There is a strong preference for approaches that are:

* Simple to implement, largely by SWD, and so that don't require new cross-team
  APIs (given the current demand for developer time)
* Simple to verify (given the importance of maintaining the OTA storage
  requirements)

Out of scope:

* Approaches that protect some non-essential packages from GC (e.g. because they
  are expected to be used again later). We do want the option to pursue these
  approaches in the future, so the chosen approach should not preclude them.
* Determining when to automatically trigger GC in response to out-of-space
  errors (e.g. in CF/full-resolver/pkg-resolver/pkg-cache vs. in fx test).

## Design

The Software Delivery stack has not yet been upgraded to conform to the
definitions and behaviors detailed in the [Package Sets RFC][rfc_0212], so the
description below uses the deprecated terms to more accurately describe the
current and proposed behavior of the system.

### Definitions

* **[Base packages][src_base_packages]**
  * The "base" or "system_image" package (identified by hash in the boot
  arguments) and the packages listed (by hash) in its `data/static_packages`
  file.
  * Generally intended to be the minimal set of packages required to run a
  particular configuration.
* **[Cache packages][src_cache_packages]**
  * The packages listed (by hash) in the "base" package's
    `data/cache_packages.json` file.
  * Generally non-base packages that we would still like to be available without
    networking.
* **[Retained index][src_retained_index]**
  * A list of packages (by hash) that were/will be downloaded during OTA and are
    used during the OTA process or are necessary for the next system version.
  * [Manipulated][src_retained_fidl] by the
    [system-updater][src_system_updater_retained] component during the OTA
    process to meet the OTA storage requirements.
* **[Dynamic index][src_dynamic_index]**
  * A mapping from package path (found in the package's `meta/package` file,
  usually the same as the path of the URL used to resolve the package) to the
  hash of the package that was most recently resolved for said path.
  * On boot the dynamic index is pre-populated with the cache packages
    (protecting them from GC as long as a package with the same path but
    different hash is not resolved later).
  * If a package is in the retained index (which identifies packages by hash)
    when it is resolved, [it will not be added][src_retained_block_dynamic] to
    the dynamic index (and so will not evict a package with the same path but
    different hash).
* **Package blobs**
  * All the blobs required by a package.
  * The meta.far and content blobs, plus the package blobs of all subpackages,
    recursively.
  * As a result of this definition, protecting a package from GC protects all of
    its subpackages. The subpackages, as packages themselves, may or may not be
    protected independently of protection provided by a superpackage.

### Current [algorithm][src_garbage_collection]

1. Determine the set of all resident blobs, `Br`
2. Pause resolution of non-resident packages
3. Determine the set of all protected blobs, `Bp`, which are the package blobs
   of the:
  * base packages
  * retained index packages
  * dynamic index packages
4. Tell blobfs to delete the set difference `Br - Bp`
5. Unpause resolution of non-resident packages

### Proposed algorithm

Create an "Open Package Index" that tracks which packages have
\[sub\]directories with open
[`fuchsia.io/[Node|Directory]`][src_fuchsia_io_directory] connections. See
https://fxrev.dev/817432 for a possible implementation (incidentally, this
implementation deduplicates the data structures used to serve package
directories, which should save at least one MB of memory). Use the open package
index in pkg-cache (the component that serves the package directories of all
ephemeral packages).

Have pkg-resolver expose an additional
[`fuchsia.pkg/PackageResolver`][src_fuchsia_pkg_resolver] capability
called `fuchsia.pkg.PackageResolver-ota`. Route this capability to
system-updater (and only to system-updater) **instead** of the current
`fuchsia.pkg.PackageResolver` capability. Packages resolved by this capability
must be in the retained index prior to resolution and will be excluded from the
open package index (by adding a flag to
[`fuchsia.pkg/PackageCache.[Open|Get]`][src_fuchsia_pkg_cache]).

Create a "Writing Package Index" that tracks which packages are currently being
written to storage. This is effectively the dynamic index except that it stops
tracking packages once they are resolved (at which point they would be covered
by the open package index or the retained index).

Use the same GC algorithm, but replace the dynamic index with the writing and
open package indices, so the protected blobs are now the blobs of the:

* Base packages
* Cache packages
* Retained index packages
* Writing package index packages
* Open package index packages

This satisfies the [requirements](#requirements):

1. *Maintain the storage usage and forward progress guarantees of the
   [OTA process][docs_ota] with the modifications from [RFC 170][rfc_0170].*

   All packages resolved during the OTA process are excluded from the open
   package index (similar to how OTA resolves are currently excluded from the
   dynamic index by first being added to the retained index), so the storage
   usage and forward progress requirements will still be met.

2. *Allow running multiple tests (each of which individually fits on the device
   and that all come from different packages) consecutively without running out
   of space.*

   When running tests from multiple different packages consecutively, GC can
   now be triggered to prevent out-of-space errors. The dynamic index used to
   protect the most recently resolved version of each test package (packages are
   added by path when resolved and only removed on reboot) and so previously run
   tests would never get GC'd, but now the open package index will stop
   protecting test packages once their last connection is closed.

3. *Not remove packages that are being consumed by components, which includes
   packages that contain running components.*

   Previously, if a component was launched and then a different version of the
   backing package was resolved, the component's package would be evicted from
   the dynamic index regardless of whether the component was still running. Now,
   since running components hold a connection to their package directory, the
   component's package will be protected from GC by the open package index.

| Index    | Package Addition Action         | Package Removal Action                      |
| -------- | ------------------------------- | ------------------------------------------- |
| Base     | Product Assembly                | never                                       |
| Cache    | Product Assembly                | never                                       |
| Retained | system-updater sets during OTA  | system-updater updates/clears during OTA    |
| Writing  | package resolution begins       | package resolution ends                     |
| Open     | non-OTA package resolution ends | last connection to package directory closes |

## Performance

The dynamic index uses memory proportional to the number of different ephemeral
packages resolved since boot (grouped by package path). The open package index
uses memory proportional to the number of ephemeral packages with open
connections (grouped by package hash). These memory footprints are both small
and similar in size due to how ephemeral packages are currently used (the open
package index will be smaller in the case where many different test packages are
run, since the dynamic index effectively leaks these entries). Any difference in
memory footprint should be smaller than the memory savings unlocked by
deduplicating the data structures used to serve package directories.

## Ergonomics

Replacing the dynamic index with the open package index makes the system easier
to understand and operate:

* The state of the open package index (and therefore the behavior of GC) depends
  only on which packages are currently in use, as opposed to the state of the
  dynamic index, which depends on the order of every package resolution that has
  occurred since boot.
* The open package index, unlike the dynamic index, does not depend on packages'
  paths (found in a package's `meta/package` file). Users are generally not
  aware of package path as a concept (they are frequently aware of the path
  component of the package URL, but the `meta/package` path can be different),
  and now GC behavior will no longer depend on it. This fixes the issue where
  unrelated packages from separate repositories but with the same package path
  would compete for GC protection (by evicting each other from the dynamic
  index). This also removes one of the last remaining dependencies on package
  path.
* Users no longer need to worry about GC deleting packages out from under
  currently executing components.

## Security considerations

No impact.

## Privacy considerations

No impact.

## Testing

There is extensive testing of the interplay between package resolution and GC in
general and OTA and GC specifically. These tests will be checked to make sure
they are still meaningful and complete.

## Documentation

The existing [GC documentation][docs_gc] will be updated.

## Diagnostics

The writing and open package indices will be exposed with
[Inspect][docs_inspect]. The base and cache packages and the retained index
are already included in pkg-cache's Inspect data.

## Drawbacks, alternatives, and unknowns

We believe this solution makes GC strictly more correct than it currently is,
based on the requirements. However, there are some unknowns and drawbacks
remaining.

### Drawbacks

The current implementation attempts to protect ephemeral packages that are not
in use currently but are expected to be resolved again, to avoid redownloading
the blobs later. The proposed implementation does not have any such protection.
This should not break any workflows because even in the current implementation
ephemeral resolution still requires network access to check the repository
metadata (meaning that the device should still be able to redownload the blobs)
and GC is triggered rarely so needing to redownload blobs should also be rare.
Additionally, the proposed approach does not prevent re-adding predictive
protection in the future.

A consequence of the previous drawback is that care must now be taken when
triggering GC in the middle of a workflow that depends on multiple packages but
that is not holding open connections to those package directories. In theory
on-target workflows should be able to hold open all required packages, but
workflows orchestrated on-host may find this more difficult.

As opposed to the current implementation, cache packages will still be protected
when a different version of the package (as identified by the path in
`meta/package`) is resolved. This means that, after resolving a different
version and triggering GC, the cache fallback (used if e.g. the network is no
longer available) will still succeed. This is bad if e.g. the non-cache version
edited config files in an unexpected way. This is acceptable because this
problem can already occur today if GC is not triggered and GC is rarely
triggered.

### Alternatives

*Instead of providing system-updater with a special
`fuchsia.pkg/PackageResolver` capability that is excluded from open package
tracking, have the system-updater close the connection to the intermediate
packages before it triggers GC.*

The open package index is updated asynchronously (whenever it notices a
connection was closed), and there is no way for system-updater to know when this
has occurred. We could create an API to query the open package index, but the
goal isn't for system-updater to unconditionally GC the intermediate packages,
the goal is for system-updater to GC the intermediate packages if the only open
connection was to the system-updater (consider the case where an intermediate
package is also a base package of the current system) and the package serving
machinery does not know who holds the client ends of the connections.
Additionally, system-updater is already manually tracking its resolved packages
via the retained index, so it is reasonable for its resolves to be excluded from
automatic tracking.

*Instead of providing system-updater with a special
`fuchsia.pkg/PackageResolver` capability that is excluded from open package
tracking, continue to provide system-updater with the standard capability and
exclude retained packages from open package tracking.*

This approach can result in packages for running components getting GC'd.
Consider the following:

1. Developer edits a test
2. System automatically stages an OTA, and the test is in the system's cache
   packages and so is added to the retained index
3. Developer runs the test, the test package is not added to the open package
   index because it is in the retained index
4. System automatically stages another OTA (possibly because the first attempt
   failed) for a different system version which removes the test package from
   the retained index
5. GC is triggered, deleting blobs out from under the running test

### Unknowns

Open package tracking protects any package with an open connection. There may be
components holding on to package directory handles longer than we expect. This
would cause issues similar to the ones seen by developers now when package
resolution fails due to out-of-space errors. Any such instances will need to be
found (which is generally straightforward using console commands like `k zx ch`)
and fixed. This will not be a problem for user devices because on user devices
only the system-updater uses ephemeral resolution.

[bug_consecutive_test]: https://fxbug.dev/42158093
[bug_repeated_test]: https://fxbug.dev/42178081

[rfc_0170]: /docs/contribute/governance/rfcs/0170_remove_binary_images_from_the_update_package.md
[rfc_0212]: /docs/contribute/governance/rfcs/0212_package_sets.md

[docs_gc]: /docs/concepts/packages/garbage_collection.md
[docs_ota]: /docs/concepts/packages/ota.md
[docs_inspect]: /docs/development/diagnostics/inspect/README.md

[src_base_packages]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/base_packages.rs;l=17;drc=2309d426b7872a187dd06a2186e80fe8b4d47cff
[src_cache_packages]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/lib/system-image/src/cache_packages.rs;l=13;drc=e4bca54d1f11e0bbd5a0d3b2affd62c4a0d19764
[src_retained_index]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/index/retained.rs;l=15;drc=5e78aa3e28edcfe9d18a2d0812ea9e8ae87b21b8
[src_retained_fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.pkg/cache.fidl;l=247;drc=f0978ea326f96b976f515d6cd7291f42c989560d
[src_system_updater_retained]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/system-updater/src/update.rs;l=687;drc=c01b2a98a7bb1c47ce98a13a208825808812564b
[src_dynamic_index]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/index/dynamic.rs;l=20;drc=5e78aa3e28edcfe9d18a2d0812ea9e8ae87b21b8
[src_retained_block_dynamic]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/index/package.rs;l=60;drc=5e78aa3e28edcfe9d18a2d0812ea9e8ae87b21b8
[src_garbage_collection]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/gc_service.rs;l=63-88;drc=4f7889bcd38b2cc65295c856f18f010bb7935bd1
[src_fuchsia_io_directory]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.io/directory.fidl;l=179;drc=de1e7ca31bf04bcdd1ded0e8ba3e1d10f9a6befa
[src_fuchsia_pkg_resolver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.pkg/resolver.fidl;l=51;drc=8238f74c32fb61405d35e46ef9e4e84d3f6c6db3
[src_fuchsia_pkg_cache]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.pkg/cache.fidl;l=14;drc=f0978ea326f96b976f515d6cd7291f42c989560d
