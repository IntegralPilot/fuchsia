<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0208" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

## Summary

This RFC describes our strategy for building and distributing
packages from [Fuchsia's platform
repository](https://fuchsia.googlesource.com/fuchsia) as an addendum
to the SDK. These packages are meant to be composed downstream using
the subpackaging mechanism.

The strategy includes the introduction of a new `.api` file format,
which is itself a new SDK API surface for Fuchsia. The format itself
and checked-in files using the format will be subject to API Council
review.

## Motivation

[Subpackages][rfc-0154] were implemented to support hermetic
nested package dependencies. A major use case for this feature is
to explicitly include specific packages along with tests out of
tree (OOT). The alternative, including references to base packages,
is being deprecated because it is prone to breakage. For example,
the dependency chain is broken if a package is removed from
the base image.

This RFC describes a strategy to commit to a set of packages that
will be distributed from the Fuchsia platform repository and how
those packages will be included as subpackages in downstream
repositories.

Note that while this RFC focuses on the specific case where these packages are
subpackaged into tests, this is a **generic mechanism** for distributing
packages via the SDK. Product owners may want to include Fuchsia-team-authored
components in their sessions in production, for example. The first packages
distributed via the SDK will likely be for tests, as we may be able to cut
corners for testing in ways that would not be acceptable in production.
Schedule considerations aside, the intent is to fully support distribution of
packages for both test and production scenarios.

## Stakeholders

_Facilitator:_

hjfreyer@google.com

_Reviewers:_

- aaronwood@google.com
- dschuyler@Google.com
- jsankey@google.com
- richkadel@google.com

_Consulted:_

- etryzelaar@google.com
- kjharland@google.com
- shayba@google.com
- wittrock@google.com

_Socialization:_

This RFC was previously discussed with representatives of Software
Delivery, Product Assembly, and Component Framework.

## Definitions

*   **Integrator Development Kit (IDK)** is the build-system-agnostic
set of code, binaries, and data files used to build programs targeting
Fuchsia, described [here](/docs/development/idk/README.md).
*   **Fuchsia Software Development Kit (SDK)** is the IDK (APIs,
tools, and language integration artifacts) [with build-system-aware
integrations](/docs/glossary/README.md#fuchsia-sdk).
One of the most notable is the [Fuchsia SDK with
Bazel](/docs/glossary/README.md#fuchsia-sdk-with-bazel).
*   **In-tree** refers to code and build rules present in the
repository at https://fuchsia.googlesource.com/fuchsia/. This
repository produces the IDK as an output.
*   **Out-of-tree (OOT)** Refers to code and build rules in
repositories that are not in-tree, and that use the SDK to produce
software and products.
*   **Fuchsia Packages** are the [unit of software
distribution][fuchsia-packages] for Fuchsia devices, referencing the
binaries/libraries, [component][fuchsia-component] manifests, and
data files necessary to execute some programs on a Fuchsia system.
*   **[Subpackaging][rfc-0154]** allows a Fuchsia Package to
reference a specific version of another Fuchsia Package as a
dependency, through containment.
*   **SDK Packages**, described in this RFC, are in-tree Fuchsia
Packages that are explicitly marked for inclusion in the IDK, and
by extension are available to SDK consumers. OOT repositories may
use the name they are published under to build against these packages
(for instance, by downloading or subpackaging them). Note that an
extension to this design may permit OOT repositories to distribute
their own packages.

## Design

The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD",
"SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be
interpreted as described in
[IETF RFC 2119](https://tools.ietf.org/html/rfc2119).

SDK Packages are produced by the in-tree build as a new artifact
referenced by the IDK. Because the IDK contains the common set of
artifacts available to the SDKs for all build systems, this provides
all Fuchsia SDK integrations the ability to reference SDK Packages.

The following stages are accomplished in-tree:

1. Selection - Choosing which packages will be distributed.
1. Validation - Ensuring the expected set of packages will be
distributed.
1. Building - Building the selected packages for all required
architectures.
1. Bundling - Producing a package directory structure
that will be included in the IDK.

SDK Packages are made available for immediate consumption in OOT repositories
through a package directory structure in the SDK at `sdk://packages/`.

The rest of this section provides detailed designs for each stage.

### Publishing SDK Packages

#### Selection

We do not want to automatically distribute all packages from the
in-tree repository for use OOT, rather, we must commit to a
specific set of packages that will be distributed along with a
platform release.

We will create a new GN template called `sdk_fuchsia_package`
requiring the following information:

* The `fuchsia_package` target to distribute.
* The SDK category for the package (e.g. "partner").
* The [API Level][rfc-0002] at which the package was added to the SDK.
* (optionally) The API Level at which the package is scheduled for
removal from the SDK.
* The list of files expected in the package and their "disposition."
Described in the next section.

This GN template will use the `sdk_atom` target to benefit from
established paths around verification / tooling of SDK primitives.

A new GN Target, `sdk_package_bundle`, will list all `sdk_fuchsia_package`
targets for the current platform API Level. This target will use the
`sdk_molecule` target to enforce bundle-level checks.

This mechanism allows platform owners to explicitly commit to
providing specific packages for OOT use for specific API Levels
(i.e. allowing for deprecating and removing a package after some
time).

As an implementation detail, we should avoid relying on GN metadata
and instead explicitly list all packages included in a
`sdk_fuchsia_package` target.

Note: We will not rely on GN metadata because it makes for a brittle
interface that is difficult to audit. Explicitly listing SDK packages
makes it easier to review CLs changing the list.

#### Validation

Distributing an in-tree package is not only a commitment to the
*name* of a package, it is also a commitment to at least some of
the *contents* of the package. This section provides background on
why this process is needed, the process of declaring a contract for
a package, and the mechanical process by which package contents are
validated at build time.

##### Background

As an example of why committing to a contract is important, consider
a package `sample-package` containing a component `sample.cm`.
`sample-package` is distributed OOT, so an SDK user may depend on
`fuchsia-pkg://host/sample-package#meta/sample.cm`. If `sample.cm`
is renamed or removed, that OOT build will now fail. This is similar
to the failure mode of an in-tree package distributed in the platform
image changing. More subtly, if `sample.cm` requires new incoming
capabilities (or any of a number of incompatible manifest changes,
see below), previous uses will fail unexpectedly.

Simply committing to the names of packages exposes us to the above
problem, but the other extreme of explicitly versioning all contents
of a package is overly burdensome. For example, if we distribute
`archivist-for-embedding`, some content of that package will change
due to changes in any one of 50 dependencies. The vast majority of
these changes do not change the `archivist` component's interface
or behavior. We therefore want to commit only to a chosen subset
of the contents of a package.

##### Contract

Ideally, the subset of package contents we validate represents the
*contract* that may be depended on by users of the package. In the
ideal case, the contract:

1. Is explicitly expressed in-tree.
1. Is precisely what OOT users of the package may depend on.
1. Is available OOT so that tooling only permits users to depend
on the contract.

Defining this "true contract" precisely is difficult and does not have a
one-size-fits-all definition. This is especially evident when
considering the contents of files such as component manifests. Some
changes to capability routing or declaration will not cause
compatibility problems (e.g. exposing a new capability) while others
generally will (e.g. depending on a new incoming capability). Other
changes' compatibilities are highly contextual (e.g. changing the
command line arguments for a component). There does not currently
exist a guide for reasoning about the compatibility of component
changes in this way.

We intend to only ship a small number of carefully vetted packages
in the SDK until we can provide clear guidance on compatibility,
subject to Fuchsia API Council review.

The rest of this section describes a mechanism for detecting changes
to an imperfect (overly general) representation of the "true contract"
for a package. This will consist of a set of present files as well
as hashes for a subset of the files, and an API Review will be
triggered if any change is detected in this set. We will publish
this representation along with the package in the SDK so that OOT
users may learn about the contract being offered, but we will permit
OOT users only to use the subset of files whose hash is included.

This means we will be slightly overly sensitive to changes in the
contract for a package (so that nothing is missed) and overly
conservative in what we allow to be used from a package (so that
the allowed usage is constrained). This gives us in-tree build-time
validation of what is being shipped in the SDK and OOT build-time
assurances that there will not be spurious runtime errors.

Note that changes to the semantics of FIDL APIs used by components
in a SDK Package are orthogonal to this validation stage. This may
be addressed using [Compatibility Tests][rfc-0015]. Future work may
also include further expanding the contract to cover referenced
FIDL files.

##### Declaring a contract

The build rule for SDK Packages, defined in the previous section,
includes a list of expected files in the package along with those
files' "dispositions." Choosing a disposition provides flexibility
in defining how each file fits in to the contract of the package.

The starting dispositions are defined as follows:

* `exact` - The exact file contents are the contract. Changes to
the file contents must be explicitly acknowledged as specified in
the next section.
* `internal` - The file is not part of the contract of the package.

`exact` matching is used for files whose contents are a contract
that must be maintained over time for compatibility (e.g. component
manifests).

`internal` means that a file is not part of the contract, but
is present in the local build.

Additional dispositions may be added when needed as extensions to
this RFC.

##### Validating the contract

We will use the familiar mechanism of `.api` golden files, which
require explicit API Reviews to change, to validate the contract
of SDK packages.

For example:

```
// sample-package.api
// The name of the file matches the name of the package with '.api' added.
// The file format is JSON.
// Note: Comments will not be present in the real .api files.
{
    // Each top-level key is a file path in the resulting package.
    "meta/sample.cm": {
        // Hash is specified for files with "exact" disposition.
        hash: "...",
    },
    "data/some-file.json": {
        // If internal, this file is simply checked for presence
        // at build time.
        internal: true,
    }
}
```

At build time, a new `.api` file will be generated based on the
input build rule and the contents of the resulting package (where
all undeclared file outputs are implicitly `internal`). If the
contents of this file do not match the corresponding `.api` file
checked in to source control under `//sdk/packages` (by deep JSON
equality), the build fails. Similar to other `.api` file mismatches,
the new `.api` file is stored as a build output and a suggested
command to copy the `.api` file into place as the new golden is
printed. This process allows expected changes to be easily confirmed
and uploaded for review.

We will document the steps for evaluating `.api` mismatches for SDK
Packages and provide guidance on what changes may be unsafe.

The `.api` file will be provided in the resulting package archive
in a later step, such that OOT tooling may use it to verify that
internal details of the package are not depended on.

This approach provides the safety of requiring API Reviews when
something meaningfully changes in a SDK Package with the
ability to define what changes are meaningful.

#### Building

The packages specified for distribution must be built for all
architectures we produce an IDK for (at time of writing, `arm64`
and `x64`). Additional flavors such as `debug` and `release` may
be introduced as needed, but initially all packages will be built
for release.

The output of this process is a set of file contents and a package
manifest for each SDK Package.

The only difference between this process and the existing build
process is that there is a separate list of package manifests
specifically for SDK Packages for use in the next stage.

Debug symbols for binaries and shared libraries included in subpackages
will be produced and uploaded to the appropriate GCS buckets. This
is the same process used for binaries and shared libraries distributed
directly in the IDK bundle.

#### Bundling

Once built, packages will be distributed in a directory structure laid out
as the following:

```
sdk://
├── blobs
│   ├── CONTENT_MERKLE_1
│   └── CONTENT_MERKLE_2
└── packages
    ├── arm64
    │   ├── 10
    │   │   ├── debug
    │   │   │   ├── PACKAGE_BAR
    │   │   │   │   ├── api
    │   │   │   │   └── package_manifest.json
    │   │   │   └── PACKAGE_FOO
    │   │   │       ├── api
    │   │   │       └── package_manifest.json
    │   │   └── release
    │   │       ├── PACKAGE_BAR
    │   │       │   ├── api
    │   │       │   └── package_manifest.json
    │   │       └── PACKAGE_FOO
    │   │           ├── api
    │   │           └── package_manifest.json
    │   └── 11
    │       ├── debug
    │       │   ├── PACKAGE_BAR
    │       │   │   ├── api
    │       │   │   └── package_manifest.json
    │       │   ├── PACKAGE_BAZ
    │       │   │   ├── api
    │       │   │   └── package_manifest.json
    │       │   └── PACKAGE_FOO
    │       │       ├── api
    │       │       └── package_manifest.json
    │       └── release
    │           ├── PACKAGE_BAR
    │           │   ├── api
    │           │   └── package_manifest.json
    │           ├── PACKAGE_BAZ
    │           │   ├── api
    │           │   └── package_manifest.json
    │           └── PACKAGE_FOO
    │               ├── api
    │               └── package_manifest.json
    ├── x64
    │   ├── 10
    │   │   ├── debug
    │   │   │   ├── PACKAGE_BAR
    │   │   │   │   ├── api
    │   │   │   │   └── package_manifest.json
    │   │   │   └── PACKAGE_FOO
    │   │   │       ├── api
    │   │   │       └── package_manifest.json
    │   │   └── release
    │   │       ├── PACKAGE_BAR
    │   │       │   ├── api
    │   │       │   └── package_manifest.json
    │   │       └── PACKAGE_FOO
    │   │           ├── api
    │   │           └── package_manifest.json
    │   └── 11
    │       ├── debug
    │       │   ├── PACKAGE_BAR
    │       │   │   ├── api
    │       │   │   └── package_manifest.json
    │       │   ├── PACKAGE_BAZ
    │       │   │   ├── api
    │       │   │   └── package_manifest.json
    │       │   └── PACKAGE_FOO
    │       │       ├── api
    │       │       └── package_manifest.json
    │       └── release
    │           ├── PACKAGE_BAR
    │           │   ├── api
    │           │   └── package_manifest.json
    │           ├── PACKAGE_BAZ
    │           │   ├── api
    │           │   └── package_manifest.json
    │           └── PACKAGE_FOO
    │               ├── api
    │               └── package_manifest.json
    └── subpackage_manifests
        ├── META_FAR_MERKLE_1
        └── META_FAR_MERKLE_2
```

Each package manifest will be put in a directory path constructed
from both the target architecture and API level:
`sdk://packages/<ARCH>/<API_LEVEL>/{debug/release}/<PACKAGE_NAME>`.
These paths will serve as stable SDK contracts that products can depend upon.
This directory will contain both the `package_manifest.json` of the package,
as well as its API file for verification purposes.

Additionally, any subpackages will have their manifests stored by package
merkle in a separate top-level directory:
`sdk://packages/subpackage_manifests/<META_FAR_MERKLE>`.

Finally, all blobs will be stored as content-addressed names
at `sdk://blobs/<CONTENT_MERKLE>`. The blobs and manifests directories
are implementation details of the packages, and may change over time. Products
should not depend on these paths to be stable.

The `LICENSES` files for all
dependencies of binaries and libraries used by those packages will be bundled
and included as part of the canonical SDK licensing story.

This directory-based structure is used for several reasons:

* Packages can be published to a repository or used as a subpackage without
the need to parse an intermediate file.
* Blobs are automatically deduplicated, even across API levels,
resulting in significant space savings (compared to archiving each
package separately) in the common case of packages containing the
same blobs (e.g. shared libraries and binaries).
* Directory structure lays groundwork for supporting multiple API levels,
as well as providing debug and release versions.
* Tools for interacting with package repositories already exist in
the SDK (`ffx` and `pm`).
* Subpackaging workflows are enabled by referencing the package manifests
directly, making it easy to publish directly into a repository.

One known downside of this approach is the case where a package is declared
as a top-level package, as well as a subpackage. In this case, the
`package_manifest.json` will be duplicated. Package manifest files are small
enough that having them listed both as a top-level, named, file and also
identified by their package merkle isn't a concern, and allows us to do a
better job of hiding the subpackaging details. The overall simplicity of
this layout outweighs this negative.

Note: The initial implementation is expected to only include packages built
with the current toolchain and latest API level. Future SDK releases will be
eventually include packages built with all compatible API levels.
Additionally, while layout will be exactly the same, the specific naming of
`sdk://packages` is TBD. There already exists a conflicting directory,
`sdk://pkg`, so naming will be resolved outside of this RFC.

### Using SDK Packages

Consuming SDK packages OOT will be as simple as consuming the package's
`package_manifest.json` relevant to both architecture and desired
API level of your system. Uses of files within the packages should be
checked against the distributed `.api` files.

This allows OOT tools and builds to properly warn about common
pitfalls, such as:

* Depending on a deprecated package that will be removed.
* Depending on a deprecated file in a package that will be removed.
* Depending on a file in a package that is not part of the explicit
contract for the package.

Implementing this functionality is the responsibility of the SDK
maintainer for each individual build system and is not described
in this RFC.

#### Including

OOT repositories may subpackage a SDK Package (R) within a
local package (L) as follows:

1. All blobs for R are stored locally (downloaded or copied).
1. L's subpackages list contains a reference to the `meta.far` blob
for R.
1. When L is published to a repository, all blobs of R (and all
blobs of subpackages of R, recursively) are included as well.

Subpackage resolution as defined in [RFC-0154][rfc-0154] will then
work for package L.

This step ensures that the package is re-published in a product-specific
repository, and it may be subject to offline blob compression and
other optimizations as part of that process. The remote TUF repository
containing the published packages MUST NOT be used directly as a
package source for a Fuchsia device (because the contained blobs
may need further processing to match the requirements of specific
products).

Note that this step will use existing package and blob publishing
tools (for example, `ffx package`), which may optimize for duplicated
blobs by performing an "insert-if-absent" operation.

## Performance

This design does not affect on-device performance.

This design may negatively impact build performance because it
results in the creation of new archives. The effects, however, will
only occur when SDK archives are created (`build_sdk_archives=true`
in GN arguments). Compared to the time spent on a full build, the
overhead of creating new archives should be negligible.

## Ergonomics

For platform owners, this design provides an idiomatic interface
to defining which packages should be distributed as part of a Fuchsia
platform release. Furthermore, the introduction of `.api` files for
this use case reduces the cognitive effort of predicting the impact
of code changes on the set of SDK Packages. Built-in automatic
checks furthermore ensure that platform owners retain complete
control over the contract represented by our SDK Packages.

## Backwards Compatibility

Publishing packages for OOT consumption is a commitment to providing
consistent semantics and naming. This RFC proposes using API
versioning to support soft migrations and provide a path to removing
or significantly changing the contract we will be committing to.

## Security considerations

This proposal allows OOT software written against the Fuchsia SDK
to directly include and reference software distributed along with
an SDK release. While the software itself is open source, care must
be taken when compiling and distributing binaries.

The process may be similar to that of distributing prebuilt binaries
in the IDK today where we provide the hash of the archive containing
the binaries we produced. Since the archive contains the hash of
an auxiliary archive containing SDK Packages, the IDK
distribution also commits to the compiled binaries provided in that
auxiliary archive.

## Privacy considerations

This design does not change the processing of user data.

## Testing

In-tree we will provide golden-file (`.api`) tests of the packages
that are going to be exposed from the tree (see "Validation" above).

We will add an OOT test to an examples repository showing the use
of a package included from the SDK. We will have an in-tree test
that attempts to mimic the behaviors of this OOT test to the greatest
extent possible; this will be an end-to-end test of the OOT stages
described above, but in terms of the underlying `ffx` commands
without Bazel support.

When [in-tree supports Bazel][rfc-0186], we will furthermore have
a test ingesting the produced SDK bundles that ensures that we can
use included tools to subpackage the SDK Packages.

## Documentation

The process for adding, modifying, and removing packages from the
SDK Package set will be documented on https://fuchsia.dev.
This includes instructions on how to configure the `.api` tests.

## Drawbacks, alternatives, and unknowns

### Alternatives to distributing packages directly in the SDK

A previous iteration of this RFC proposed a mechanism of
archiving and uploading SDK Packages separately through CIPD,
decreasing any size changes to the SDK bundle. Similarly, SDK users
do not need the package distribution, so having an optional archive
means those users could avoid downloading it entirely.

While great for future efforts, this pattern is not standard to the
SDK build, and would require novel build tooling for enforcing SDK category,
`.api` tests, as well as novel tooling for downloading and using these
SDK Packages OOT.

To address size concerns for the SDK, only a small initial set of SDK packages
for enabling test cases will be included. For example, these may be considered
for inclusion:

* driver_test_realm
* archivist-for-embedding
* log-encoding-validator
* realm_builder_server
* test_ui_stack

These packages built as a release build for x64 add up to approximately 43 MiB
uncompressed, or 18 MiB compressed with the default gzip settings. This seems
reasonable compared to the current SDK at 700 MiB compressed.

Any additional packages included will be subject to Fuchsia API Council review
for both content and size changes. Additionally, Future iterations of this
framework may point to remote-stored blobs to further help on concerns of
size storage.

### Alternatives to using a directory structure

Instead of using a directory structure for enforcing architecture and API
level, a file at the top level could present the same information in a JSON
format. A benefit to this design comes from malleability of the file, compared
to altering a directory structure. However, parsing such a file would require
additional SDK tooling, and benefits of this are not present today.

Future iterations of this tooling may shift away from a directory structure,
and required tooling will be built and provided should this case arise.

## Prior art and references

Fuchsia Software Delivery is a new approach to updating software
systems, a space with much prior art described further in these
documents:

- [Software Delivery Overview][fuchsia-swd]
- [Software Delivery Goals][rfc-0139]

[fuchsia-component]: /docs/concepts/components/v2/README.md
[fuchsia-packages]: /docs/concepts/packages/package.md
[fuchsia-swd]: /docs/get-started/learn/intro/packages.md
[rfc-0002]: /docs/contribute/governance/rfcs/0002_platform_versioning.md
[rfc-0015]: /docs/contribute/governance/rfcs/0015_cts.md
[rfc-0154]: /docs/contribute/governance/rfcs/0154_subpackages.md
[rfc-0186]: /docs/contribute/governance/rfcs/0186_bazel_for_fuchsia.md
[rfc-0139]: /docs/contribute/governance/rfcs/0133_swd_goals.md
