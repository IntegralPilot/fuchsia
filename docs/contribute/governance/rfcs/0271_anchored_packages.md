<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0271" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Problem Statement

Currently, the system update ("OTA") implementation in Fuchsia requires storing
two complete set of blobs at the same time. This is straightforward but poses a
problem for devices which suffer from a lack of disk space.

## Summary

This RFC proposes an implementation for _anchored packages_, initially described
in [RFC-212]. These package sets satisfy the following invariants:

* Only the _permanent, anchored_ packages (bootfs, base as per [RFC-212]) are
  downloaded during the system update (OTA) itself.
* The packages and their blobs are uniquely identified by their Merkle hashes.
  For _automatic anchored_ and on _demand anchored_ packages, the `meta.far`
  archive hashes, carrying this information, are included in the update package.
* The _automatic anchored_ and on _demand anchored_ packages are downloaded at
  some point after the system update has been applied, and the device has booted
  into the new system version.
* The previous point is especially disjunct from _eager updates_ as per
  [RFC-145], which map to the set of _automatic, upgradeable_ and _permanent,
  upgradeable_ packages as per [RFC-212]: The _anchored_ packages do not
  delegate the authentication of what constitutes the "correct" package to
  another party: _Eager updates_ allow for packages to be updated independently
  of the system OTA, while _anchored packages_ do not.

The invariants yield the following implications for the sets of _anchored
packages_:

* They cannot be updated to newer versions without performing a system update,
  since the cryptographic hashes of all packages are still distributed with the
  system update.
* From the viewpoint of an application, it shall not matter if the packages are
  distributed with the OTA or as an anchored package. The application may,
  however, experience some delay when accessing functionality that is provided
  by packages not yet resolved. The details on this are given in the section on
  Performance.
* _Automatic anchored_ packages are not downloaded on demand, unlike the
  packages from TUF repos in engineering builds and the _on-demand anchored_
  packages. Instead they are downloaded once booting into the updated system has
  started.

_Automatic anchored_ and _on-demand anchored_ packages can be leveraged to avoid
the need to keep every package in the system stored twice during an OTA on the
device's non-volatile storage, hence improving storage efficiency, especially on
storage-constrained devices.

## Motivation

The commonly employed system update mechanism on production Fuchsia systems
currently performs complete updates over a three partition "ABR" scheme.
Partitions A and B each contain a full system image. The R partition contains a
minimal recovery partition which is not in scope for this RFC.

One of the partitions A or B is the active partition at any point in time, i.e.
the boot loader will boot the system into the active partition when the device
is turned on. Currently, for the _user_ buildtype, all software known to
the package management is always available on-device.

When an OTA is performed, the system updater will write a complete new system
image onto the non-active partition A or B. Also, it ensures that all known
blobs are downloaded to the fxblob/blobfs storage which is shared by both A and
B slots. Upon successful completion, it will flip the bit that indicates which
is the active partition, and reboots into the new system image. This means that
at any given time, one full system image is occupying space but is not used at
all, and there is a time window during which fxblob/blobfs must hold the blobs
for both the currently running version of the device software, and the new
version. This allows for a more efficient usage of a device's resources.
[RFC-212] has defined and generally laid out the nomenclature and concepts for
extensions of the Fuchsia package management system to reduce the footprint of
the system images. It does so by allowing packages to be excluded from the
system image and to be downloaded to the fxblob/blobfs storage after the system
update. This removes the need for a time window where the old and new version of
a package both occupy space on fxblob/blobfs.

An additional motivation to support anchored packages is the desire to minimize
the amount of unnecessary network traffic. If a majority of the users of a
Fuchsia-based system do not use certain software components at all, excluding
them from the OTA and only downloading them on demand might reduce significant
amounts of data to be transferred, reducing the required capacities on servers
and networking.


## Definitions

The following terms and definitions apply as defined in [RFC-212]: Base, Bootfs
packages, Cache, Eager update, Garbage collection, Package, Package set, Package
version, Parent package, Permanent package, Product, Product build, Product
release, Subpackages, System update, Universe, Versioning authority.

## Stakeholders

_Facilitator:_

jamesr@google.com

_Reviewers:_

* etryzelaar@google.com (SWD)
* amituttam@google.com (PM)
* awolter@google.com (Software Assembly)
* markdittmer@google.com (Security)
* gtsai@google.com (Infrastructure)

_Socialization:_

This RFC was discussed in a series of design discussions internal to the
software delivery team. Early versions of the text in this RFC were reviewed
with stakeholders in the Software Delivery, Build and Assembly teams, and
potential customers.

## Requirements

* The delivery mechanics for anchored packages shall not require additional
  services or infrastructure, i.e. the same mechanism of getting the system
  update (OTA) onto the target device shall be applicable to the anchored
  package as well.
* The specification which packages belong to which anchored package set is done
  manually. No mechanism that attempts to automatically determine which package
  could be part of which package set is going to be implemented based on this
  RFC.
* The implementation is scoped to the limits as defined per [RFC-212] and
  displayed in the illustration below. Specifically the following scenarios
  will **not** be covered or supported by _automatic_ or _on-demand anchored_
  packages:
  * Support for delegation of version control.
  * Support for packages that are essential for the out-of-box-experience (OOBE)
    workflow of the product.
    * This means that if packages essential for OOBE are marked as _automatic_
      or _on-demand anchored_ packages, the package management stack has no
      way to guarantee that all packages needed for a successful OOBE workflow
      are available when needed. This is due to the fact that in many cases, the
      OOBE workflow includes setting up networking. Hence, the _automatic_ and
      _on-demand anchored_ packages are not guaranteed to be resolvable before
      the OOBE completes.

![Package sets overview][diagram-1-package-sets]

## Design

### The _anchored_ package sets

A _package set_ is a collection of packages, all of which are delivered and
updated using the same strategy. The most frequently used package set in
production devices are the _permanent_ (_base_ and _bootfs)_ sets which share
the main characteristics of being always available and set by the product
definition:

* The Merkle hashes describing the packages, ensuring their integrity,
* and the packages themselves

are downloaded as part of the OTA update process, _before_ the reboot into the
updated product version. Because of this, combined with Fuchsia's default [ABR
partitioning scheme][abr_scheme] in which both the A and B partitions contain a
fully bootable system, packages of the base set are stored potentially twice in
fxblob/blobfs: If they are different, one version from the product in the A
partition is in the store, and one from the product in the B partition. From a
running system, the list of packages in the base set are reflected by the
`data/static_packages` file of the base package. In the process of package
resolution, these packages are resolved by the [base resolver][base_resolver],
a part of `pkg-cache`. In other words, they are resolved locally and do not
require network connectivity.

The _automatic and on-demand_ _anchored_ package sets share some characteristics
with the bootfs and base sets, but are distinct in fundamental ways:

* Like with base packages, the Merkle hashes describing the anchored
  packages are downloaded to the intended system partition before the reboot
  into the new system version.
* Unlike with base and bootfs packages, the blobs themselves are not downloaded
  to fxblob/blobfs before the reboot into the new system.

This allows the optimization of the download stages of an OTA update for storage
size, since it can be avoided to keep multiple versions of a package in the non
volatile storage. This is described in "Boot sequence with automatic anchored
packages" below.

The distinction between the _automatic_ and _on-demand_ _anchored_ packages lies
within the trigger that causes the package resolver to fetch the package:

* An _on-demand anchored_ package is fetched as soon as any eligible component
  with the permission to query the resolver on the running Fuchsia system
  requests resolution of the package.
* An _automatic anchored_ package is resolved as early as possible after
  rebooting into a new Fuchsia system after the OTA has been performed.

### Garbage collection of anchored packages

This RFC proposes that, until a potential revision of the garbage collection
algorithm in the future, the following rules apply for the collection of
anchored packages:

* The _automatic anchored packages_, once downloaded to fxblob/blobfs, will be
  protected from garbage collection until the next reboot into a newer system.
* Only when the system version and thus the Merkle hashes have changed, and they
  are no longer part of the then-running system version, will the _automatic
  anchored_ packages be subject to garbage collection.
* The _on-demand anchored packages_ will receive no additional protection from
  garbage collection, and will follow the rules specified in
  [RFC-217] on open package tracking.

### Resolution of _on-demand anchored_ packages

The package resolver already has the functionality to download packages and make
them available on the running system on-demand. Hence, leveraging this feature
for on-demand anchored packages is straightforward: As soon as the resolution of
a package that is not available on the system, the package resolver will
download the blobs, they will be verified and made available via fxblob/blobfs.

The main difference between the _always available_ package sets and _automatic /
on-demand_ _anchored_ packages is that for the latter two, the resolver requires
an internet connection.

_On-demand anchored_ packages do not have to be explicitly marked as such from
the viewpoint of the running system. When a package is requested, the resolver
will simply notice that its blobs are not available in the package cache and
initiate the download process.

However, the decision of whether a package is included in the product as an
_on-demand anchored_ package falls to the developer, and marking it as such in
the product definition is required:

* The Fuchsia product definition will accept a list of `on_demand_anchored`
  packages.
* The `ffx assembly product` tool will be able to emit the list to the image
  assembly configuration.

During product assembly:

* The package (blobs and manifests) are included in the main product bundle.
* The blobs are _not_ included in the fxblob/blobfs used for flashing a device.
* The list of packages in the on-demand anchored set is constructed in a way
  analogous to the base packages. The list will be part of the
  `AssemblyManifest` and included in the update package. A running Fuchsia
  system will find the list of packages of this set in the
  `data/anchored_packages` file of the base package.

### Resolution of _automatic anchored_ packages

Like with _on-demand anchored_ packages, the decision of whether a package is
included in the product as an _automatic anchored_ package falls to the
developer, and marking it as such in the product definition is required:

* The Fuchsia product definition will accept a list of `automatic_anchored`
  packages.
* The `ffx assembly product` tool will be able to emit the list to the image
  assembly configuration.

During product assembly:

* The package (blobs and manifests) are included in the main product bundle.
* The blobs are _not_ included in the fxblob/blobfs used for flashing the
  device.
* The list of packages in the automatic anchored set is constructed in a way
  analogous to the base packages. The list will be part of the
  `AssemblyManifest` and included in the update package. A running Fuchsia
  system will find the list of packages of this set in the
  `data/anchored_packages` file of the system update package.

The packages in this set are resolved after rebooting into a new or upgraded
Fuchsia system. As mentioned before, this is notably different from the packages
in `data/static_packages`, all of which are always present and are known to
`pkg-cache`.

### Boot sequence with automatic anchored packages

The boot sequence of a system which has the set of automatic updated packages is
as follows:

* Following an initial boot of a new or a reboot after an OTA, when network
  connectivity is confirmed:
  * An fxblob/blobfs garbage collection will be performed to evict the dangling
    blobs present from the previous system version.
  * In the event of a lack of network connectivity, the garbage collection is
    delayed until the network becomes available.
* The boot sequence completes as usual, having all packages of the base set
  already available.
* While the boot sequence completes, the package resolver will check for the
  presence of each of the blobs of the _automatic anchored_ set, and start
  downloading all its blobs not present in fxblob/blobfs.
* In case download errors occur, the resolver will continue to check for missing
  blobs from the _automatic anchored_ set and retry downloading the missing ones
  as the system accrues uptime.

### Possibility of unavailable packages

Unlike on a system where only _permanent_ packages are available, as soon as
_on-demand_ or _automatic anchored_ packages come into play, there is no more
guarantee that all packages will be available at all times.

The boot process will by default not block on download failures or slow
downloads, unless a product explicitly requires this behavior and implements it
correctly. There are two main reasons:

* A device may not even be configured for network connectivity yet, and has to
  go through the out-of-box-experience (OOBE) workflow, before _on-demand_ or
  _automatic anchored packages_ can be downloaded in the first place.
* Potentially unfavorable user experience. For instance, a device could suffer
  from internet connectivity loss during the download of the set of _automatic_
  _anchored_ packages, while the user wants to use a simple function of the
  device which is readily available through the _base_ package set.

Note that the possibility of at least intermittently unavailable packages
mandates that the components depending on them must be able to handle this case
gracefully. Specifically, calls to components which might need to be fetched via
network are not guaranteed to complete in a specific amount of time as network
latencies and speed are unknown for most use cases. More details are discussed
in the section "Reporting of the availability of packages".

### Reporting of the availability of packages

Compared to the situation where all known packages are always available on the
device, the post-OTA download and the benefits of OTA storage savings come at
the price of predictability. This might give rise to related requirements, for
instance:

* Components might want to know if a dependency is readily available or whether
  it is still downloading before calling a particular function.
* Especially user-interactive application might want to implement a
  mechanism to watch for unexpectedly long wait times when calling into a
  provider of some functionality, and display a message to the users to manage
  their expectations.
* Tracing or debugging tools might want to inspect detailed data or metrics on
  package availability, such as whether a package is present, being downloaded,
  at what download speed, latency, retries, and so on.

The specific detailed requirements and the implications for the resulting
changes to the Fuchsia platform are not comprehensively known at the time of
writing this RFC. Thus, a detailed design for this type of monitoring and
availability reporting is out of scope. This might be addressed in a future RFC
or implementation.

Following current planning, tooling, for instance `debug_agent` or `zxdb` are
likely the first components to make use of some form of availabily reporting and
monitoring. This will allow for a better understanding of the requirements, the
gaps and the necessary changes to be addressed in the future. Complex use cases
like the interaction with interactive UI, progress reporting to end users and
similar cases are not within scope of this initial set of use cases.

## Implementation

The implementation will be split in multiple phases to allow for synchronization
between participating teams and validation of the functionality before enabling
the functionality for Fuchsia-based products.

The first phase will encompass the implementation of the pure functionality in
the package resolver and package cache, which requires:

* SWD
  * Support for obtaining the list of _automatic anchored_ packages from the
    base package's `data/anchored_packages` upon startup, and making the blobs
    for the _automatic anchored_ packages available.
* Build, Assembly
  * Support for processing a list of desired _automatic_ and _on-demand
    anchored_ packages during product build, and providing
    `data/anchored_packages` to the images.
  * Update the size check tooling to support anchored packages and reflect the
    build and image sizes correctly.
* Security
  * Technical review of the internet facing code path of the resolution of the
    _automatic and on-demand anchored_ packages.

The second phase will encompass the preparation of enabling the new
functionality for Fuchsia-based products. This requires:

* SWD
  * Preparation of a variant of an existing product configuration that moves
    some packages from the base set to the _automatic or on-demand anchored_
    set.
* Server side packaging infrastructure
  * Storage for packages and integration with the blob store.

The third phase, would see the adoption of _automatic_ or _on-demand anchored_
packages into a Fuchsia-based product with a first application. This decision is
out of scope for the RFC. However, conceptually, this would require:

* Selected application / maintaining team and test team
  * Review and adapt the application to be able to handle if dependencies are
    temporarily unavailable.
  * Create and adapt tests and test plans to simulate and run through the
    possible set of error states that can arise (slow download, DNS issues,
    interrupted connectivity, out-of-box experience, ...).
* Release management
  * Ensure there are end-to-end product tests in place which guarantee the OTA
    and OOBE workflows do not depend on _on-demand_ packages.
  * Shepherding the release that incorporates anchored packages through all
    stages and final rollout to the users of the Fuchsia-based product.

### The data/anchored_packages file

A central element of the implementation is how the system identifies the
_automatic and on-demand anchored_ packages. As mentioned before, they will be
listed in `data/anchored_packages`, similar to the `data/static_packages` file.
However, unlike the simple key/value format of the `static_packages` file, this
will convey more information for automatic and on-demand anchored packages, in
the beginning:

* Whether they are _automatic_ or _on-demand_.
* The package, identified by its URL.
* The package hash.

Hence, the `data/anchored_packages` file will be a json file, containing this
information, conceptually:

```
{
  "on_demand": {
    "fuchsia-pkg://fuchsia.com/package1": {
      "hash": "d5eb4bb8a394176f20a266bac7cd8a077579f57119c5e1ed0981cd5ea563c183"
    },
    "fuchsia-pkg://fuchsia.com/package2": {
      "hash": "d33da6cb5fc8787ae1386e85ff08a32c6e846668d5eb4bb8a394176f20a2681c"
    }
  },
  "automatic": {
    "fuchsia-pkg://fuchsia.com/package3": {
      "hash": "d189658d88636999abe2c4375409ace94fd0408d338e96692b8b8bad07c484d4"
    },
    "fuchsia-pkg://fuchsia.com/package4": {
      "hash": "e6c22775bac3301f18d012493ffc6c7fe442b59cd5eb4bb8a394176f20a266ba"
    }
  }
}
```

## Performance

The performance implications of this RFC can split into the following areas:

1. The performance of a Fuchsia-based product at runtime for packages that are
available on fxblob/blobfs. We do not expect noticeable changes in runtime
performance in this area, since for resolved packages of the anchored set there
is no difference to base packages with regard to how they are provided by the
package cache or how they are run.

1. The performance of a Fuchsia-based product at runtime for packages that are
known to the package management but not yet downloaded. The overall performance
impact of the resolution (download, verification) of packages on the
Fuchsia-based product in terms of CPU and memory consumption is expected to be
benign. The impact _may_, however, be noticeable in situations where the user of
the product wants to execute functionality that depends on packages still being
downloaded. In this case, the user experience is impacted to a certain extent,
depending on the download speed. The extent is dominated by the speed and
latency of the user's internet connection, hence it is out of scope for this
RFC. Note that the on-device application depending on the package still being
downloaded could clarify the situation to the user, for instance by conveying a
message that a required download is still in progress. From a UX perspective,
this is preferable to a device that appears unresponsive or in an unknown state.

1. The overall duration of the OTA process changes for products that make use of
_automatic anchored_ packages. We expect that - on average - the time between
"download of the system update package has started" and "the new build is
running" is reduced, the average time between "download of the system update
package has started" and "all packages have been resolved" increases. As
mentioned in the previous bullet point about resolving packages which are not
available on fxblob/blobfs yet, it is advisable that the software depending on
_non-permanent_ packages is ready to manage the user's expectations. For some
types of devices this change in duration might be less of an issue, e.g. devices
that are routinely updated in the middle of the night when they are unused.

1. The package serving infrastructure will most likely experience different load
patterns when serving to products employing _automatic_ or _on-demand anchored_
packages, compared to products that distribute updates as a single OTA. This
depends on, for instance, when devices reboot after receiving the update package
in case of _automatic anchored packages_, or when some functionality is first
requested by the user in case of _on-demand anchored packages_. But also, other
factors like geographical location and whether certain functionality is employed
at all by users will cause the load of the package serving infrastructure to be
different compared to serving to products with only static packages. Aspects of
this are discussed in the section "Future work".

1. Due to the previous points, it is advisable to have metrics for (i) the
duration of the download of the update package and its installation, and (ii)
additionally for the duration of the resolution of the _automatic anchored_
packages after the first reboot into the new build. This allows for better
understanding how frequently the user experience suffers from flaky internet
connection or slow download speeds after the system update has already been
applied successfully.

### Benchmarks

Products leveraging anchored packages may experience differences in user visible
activities. Thus, for such products we recommend collecting additional
benchmarks routinely to ensure the user experience does not regress and to
counteract if the benchmarks suggest:

* The time to resolution of the set of _automatic and on-demand anchored_
  packages for a given product under a defined variety of network conditions
  (uplink / downlink speed, latency, package drops, etc.).
* The frequency of unsuccessful resolution attempts of anchored packages, again
  under a defined variety of network conditions.
* The frequency of how many times users Fuchsia-based products request
  functionality which depends on packages which are not yet resolved.

Those metrics are intended to provide data to make a reasonable decision whether
a certain package can be moved out of the base set or whether an anchored
package should be moved back into the base set.

## Ergonomics

### Eng workflows

The engineering workflows would stay largely the same as what can be
accomplished with eng builds today, where packages are made available
dynamically from a repository server, serving a TUF repository. Generally
speaking, a component should not have to deal with the lower level details of
how its dependencies are made available, and the package resolution process
abstracts these details away.

We expect that we will extend the existing tooling around `ffx` to support test
and development workflows around anchored packages if and when gaps are
identified.

## Backwards Compatibility

This work will be backwards compatible with existing uses of the Software
Delivery stack. The package URL namespace structure or resolution concepts are
not going to change.

This work does not deprecate existing functionality. Thus unless anchored
packages are used, the changes are entirely transparent.

## Security considerations

There are security considerations with this proposal. They overlap slightly with
[RFC-145] but in a fundamental way this proposal is more benign.

* Like RFC-145, we resolve packages at system runtime, i.e. the package resolver
  needs permission to connect to the internet, at least until all packages are
  resolved and the blobs are stored in fxblob/blobfs.
* Unlike RFC-145, this proposal does not delegate authority about the set of
  runnable packages. All hashes of the entire package universe are known
  upon boot into the new OTA. Hence, product definition and what constitutes
  valid packages are identical to a product that only uses base packages.

## Privacy considerations

We believe the proposal does not include any new functionality with an impact on
privacy. However, the following point should be considered:

We recommend the introduction of new metrics earlier in the section on
Benchmarks. Those metrics are intended to cause user experience regressions. The
introduction of such benchmarks, if collected from production devices, shall go
through privacy reviews.

## Testing

The new extensions to the software delivery stack are going to be covered by
extensions to the existing test suite and will cover unit tests, integration
tests and e2e tests covering all new code paths.

Testing will also cover scenarios where resolution of anchored packages depends
on a successful prior garbage collection and facing close-to-full-disk
scenarios.

To evaluate the UX of Fuchsia-based products leveraging anchored packages,
validation of critical user workflows should be performed, especially under
suboptimal network conditions.

## Documentation

Upon acceptance of this RFC we will:

* Update the glossary entry for "[package][glossary-package]".
* Update the section on packages in "[concepts][concepts-package]".
* Update the page on "[packages in the IDK][idk-packages]".
* Update the page on product packages in the
  "[get started guide][guide-packages]".
* Add information about how updates are configured when using anchored packages.

## Drawbacks, alternatives, and unknowns

### Alternative: Using other partitioning schemes for Fuchsia-based products

This alternative does not encompass the same semantic scope as the previous
focus on the software delivery mechanisms, and is largely orthogonal to this
RFC. But it focuses on one of the important reasons for _automatic and
on-demand_ anchored packages: To save space on the non volatile storage device
of a Fuchsia-based product.

It is frequently assumed that those products are built on an ABR partitioning
scheme:

* A and B as partitions into which an entire system image fits
* R as the recovery partition for emergencies, when both A and B are unusable

While it is a proven concept which is used in a plethora of devices of varying
types, it is certainly not the only option. There are at least two more
possibilities which save close to half the space:

* AR-scheme: There is only one system partition (A). OTAs are performed by
  booting into the recovery image (R), and then the system partition (A) is
  overwritten.
* AB-scheme: Instead of having a separate partition for recovery, both A and B
  contain recovery software that can be used to re-flash the other system
  partition.

While those alternative partitioning schemes address the goal of providing more
free disk space on constrained devices, they are very different from focusing on
the packages themselves, to the point where they can be regarded as orthogonal
from the viewpoint of space savings. Anchored packages, however, can satisfy
other goals as well, such as reducing network traffic, hence changing the
partition scheme is an option that should be evaluated independently of this
proposal.

## Future work

### Removal of the data/static_packages file

We might choose to depreciate and remove the `data/static_packages` file and
include the packages listed in that file with the more versatile and extensible
`data/anchored_packages` file.

### Notification API

Intentionally out of scope for this RFC, we might choose to implement some form
of a notification API in the future, where components could inquire, for
instance, about the state of package resolution, download speed or estimated
time of download completion.

### Inclusion of package content blobs in the system update package

Depending on future requirements, we might choose to include not just the
packages' `meta.far` files in the update package, but also package content
blobs. This may, for instance, allow the estimation of download times for
_automatic and on-demand anchored_ packages.

### Optimization for specific workflows

With package sets, there is potential to allow optimization for specific
workflows. However, these optimizations might come at extra costs or may be
complicated to implement, especially when considering all edge cases. Thus,
these considerations are out of scope for this RFC. But to provide the context,
some of the potential optimizations are:

* Aggressively leveraging _on-demand_ packages allows to work with fxblob/blobfs
  storage that is smaller in size than the combined set packages of a defined
  product. For such a case, _on-demand_ packages could be frequently subjected
  to garbage collection and re-download of the packages which need to be in use.
  While possible, it would require careful design of the possible workflows on
  such a product to guarantee there is always sufficient storage to hold all
  packages in use at any given time.
* It would be possible to include the _automatic_, or even (most of) the
  _on-demand anchored_ packages in the initial fxblob/blobfs image used for
  flashing, hence eliminating the need to download those packages after the
  initial out-of-box-experience. While possible, it introduces subtle
  differences between the initial image and subsequent OTAs, with likely
  complications for assembly and testing.

### Additional developer tooling

With the inclusion of _automatic_ or _on-demand anchored_ packages, there is
potential to support developers with additional tools. The decision of which
tools to build is out of scope for this RFC, but some potential tools are:

* Report and pretty-print the package dependency graph, highlighting the
  direct and transitive dependencies to not-always-available packages.
* Simulation of non-perfect network conditions when serving packages to
  development targets via `ffx repository serve`, for instance slow networks,
  or intermittent connection issues.

### Revisiting of the package serving infrastructure

Based on the networking metrics that may be collected from products employing
_automatic_ and _on-demand anchored_ packages, teams operating package serving
infrastructure may gain insights that help optimizing the serving infrastructure
for shorter download times or latency for the users. Expected aspects are, for
example:

* Distribution of storage buckets for the package artifacts across
  cloud regions.
* Load balancing to minimize the access latency for the clients,
  using methods like IP geolocation to determine the closest package server.
* Dynamic scaling of the number of instances of package servers during times of
  rolling out larger updates.
* Anticipation of the expected load patterns before minor and major releases.


## Prior art and references

* [Packages in fuchsia.dev][concepts-package]
* [Eager package updates RFC][RFC-145]
* [Subpackages RFC][RFC-154]
* [Early boot packages RFC][RFC-167]
* [Package sets RFC][RFC-212]

<!-- Links -->
[RFC-145]: /docs/contribute/governance/rfcs/0145_eager_package_updates.md
[RFC-154]: /docs/contribute/governance/rfcs/0154_subpackages.md
[RFC-167]: /docs/contribute/governance/rfcs/0167_early_boot_packages.md
[RFC-212]: /docs/contribute/governance/rfcs/0212_package_sets.md
[RFC-217]: /docs/contribute/governance/rfcs/0217_open_package_tracking.md
[abr_scheme]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/firmware/lib/abr/README.md;drc=888696eccfc943a67b1e78440d55a3b05f720f98
[base_resolver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/pkg/bin/pkg-cache/src/base_resolver.rs;drc=28016ef5e1da668960dd625501c0d18e4c00b07c
[concepts-package]: /docs/concepts/packages/package.md
[diagram-1-package-sets]: resources/0271_anchored_packages/diagram_1_package_sets.png
[glossary-package]: /docs/glossary/README.md#package
[guide-packages]: /docs/get-started/learn/build/product-packages.md
[idk-packages]: /docs/development/idk/documentation/packages.md