<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0276" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- mdformat on -->

## Problem Statement

In Fuchsia's current implementation, there is no mechanism to guarantee that all
blobs in fxblob/blobfs satisfy all relevant properties before commencing an OTA.
Specifically, blobs with a given hash existing on-disk are not guaranteed to be
overwritten if an incoming blob has the same hash, even if it would require less
space. This makes it difficult to reason about space requirements when working
with very little headroom.

## Summary

This RFC proposes a way to support evolving on-disk changes to the blobs during
an OTA. It enables fxblob/blobfs to report if the stored blob is in the desired
format, and accept an overwrite to replace the blob if it is not. This supports
blob details other than just the verified contents to be used during a system
update to unify the data on disk without ambiguity. This RFC proposes the
following key changes:

*   Extension of fxblob/blobfs to report if each blob is in the desired format.
*   Bump of the delivery blob type to v2.
*   A stepping stone for the introduction of the new delivery blob type.
*   Server-side support for the introduction of a new on-disk blob format.

## Stakeholders

_Facilitator:_ jamesr@google.com

_Reviewers:_

* jfsulliv@google.com (Storage)
* etryzelaar@google.com (SWD)
* amituttam@google.com (PM)
* awolter@google.com (Software Assembly)
* markdittmer@google.com (Security)
* gtsai@google.com (Infrastructure)

_Socialization:_

This RFC was discussed in a series of design discussions internal to the
software delivery and local storage teams. Early versions of the text in this
RFC were reviewed with stakeholders in Build and Assembly teams, as well as
Infrastructure.


## Requirements

*   For every client device in production, a soft transition to new blob formats
    shall always be possible. That is, any client device in production shall be
    able to be migrated to a new blob format without any user-visible impact.
    *   Note that the format of the non-delivery-blobs is not going to change
        as a result of this RFC being accepted. No changes to the computation of
        the Merkle roots for given blobs are planned.
*   The number of stepping stones required for the introduction of blob format
    changes should be minimal.
*   The mechanism used to update the format of existing blobs shall support
    pause and resume between blobs, and they shall support both intended and
    unintended interruptions, for instance:
    *   Intended: Controlled by the software / configuration, for example to
        manage device thermals, read/write performance requirements, testing and
        benchmarking.
    *   Unintended: Loss of power, device reboot.

## Design

The following sections describe the technical changes to facilitate the given
requirements.

### Storage filesystem reporting of blob state

The on-device blob filesystems will decide internally based on written formats
of the blob if the blob is not in the desired format, meaning it would accept an
overwrite to attempt to replace the blob. The behaviour will not necessarily be
consistent between different blob filesystems such as blobfs and fxblob.

### Introduce delivery blob type "2"

In [RFC-207 ("Offline blob compression")][RFC-207], the concept of _delivery
blobs_ is introduced. Delivery blobs are a format suitable for efficient
delivery of blobs from a blob server to fxblob/blobfs on a device. Delivery
blobs contain not only the raw data but also metadata which is shared among
software delivery and local storage. This metadata can indicate, for instance,
the type of compression of the payload.

At the time of writing of this RFC, the most recent delivery blob type is 1.
With the introduction of storage of metadata about blobs (see above), we are
going to introduce delivery blob type to 2 ensuring that there is a clear
semantic relationship: The new delivery blob type will use a different set of
compression settings targeting a higher overall compression ratio as compared to
delivery blob type 1.

The consequences of this are:

*   The Fuchsia platform code (local storage, package management, ffx) will be
    able to handle both types 1 and 2.
*   Type 1 blobs will not go away immediately for production devices. For them,
    a stepping stone has to be completed first, making sure that no older system
    attempts to download type 1 blobs that do not exist anymore.
*   For the local development workflows involving TUF repositories, there might
    be situations where `fx ota` will not work when assembly switches from
    producing type 1 to type 2 blobs, because there is currently no TUF-based
    mechanism to force a multi-stage update through a stepping stone release.
    For these situations, developers will be expected to `ffx target flash`
    their devices as a workaround.

 As soon as assembly has been changed to create type 2 blobs:

*   The product bundle will contain only type 2 delivery blobs by default,
    placing them in the `blobs/2` subdirectory.
*   The server side package serving infrastructure will make both the 1 and 2
    blob types available to client devices.
*   The system updater will begin to request type 2 blobs.

After the stepping stone build has been established:

*   The server side package serving infrastructure will no longer guarantee type
    1 blobs are available to client devices for builds after the stepping stone.
    The step-by-step process of migrating the production builds through the
    stepping stone build is described in the section
    [Implementation](#implementation).

To clearly determine if a blob requires downloading and rewriting during an
update, introducing delivery blob type 2 is essential. Previously, the package
cache only considered a blob's presence, not its format. If the blob format
changed while the delivery type remained 1, the package cache couldn't
differentiate between "1 with old blob format" and "1 with new blob format" on
the server. Introducing type 2 eliminates this ambiguity, as "1 with new blob
format" would then not exist.

#### Storage size concerns for type 1 blobs and the migration to type 2 blobs

One particular reason for the migration to type 2 delivery blobs is to enable
development and product teams to accurately reason about the OTA size
requirements on devices where storage is very tight.

The background is as follows: The names of the blob files are directly derived
from their raw content. This applies to both non-delivery blobs and delivery
blobs. This means, for example, changing the compression settings for blobs
during assembly would result in blobs with different sizes than before but
identical names. For the blobs that already exist on a device, the package
cache would not attempt to download them again, even if their size has changed
on the server side. This makes it next to impossible to accurately reason about
the _exact_ size requirements for an OTA. Since the introduction of type 2 blobs
goes alongside the introduction of on-disk blob metadata, as introduced above
and detailed below, reasoning accurately will become possible for all blob types
later than type 1.

To guarantee that the first migration (from type 1 to type 2) will not go over
the size budget during an OTA, a scheme like the following shall be employed to
Fuchsia-based products:

*   Before the OTA that will switch to type 2 blobs, it is verified that the
    worst case still fits onto the disk, i.e. the type 1 blobs that have been
    downloaded during the last OTA and are currently stored on-disk, plus the
    new type 2 blobs that will be downloaded to facilitate the migration. This
    is routinely ensured by already existing automated size checks in Fuchsia's
    build infrastructure.
*   The compression settings in the first build that comes with type 2 blobs
    will result in smaller blobs.
*   This ensures that:
    1.  The blobs will definitely fit onto the devices during the migration OTA.
    1.  After the next garbage collection of the blob filesystem, the blobs will
        take _less_ storage than the type 1 blobs would have, guaranteeing a net
        win of free space.
    1.  For every blob, its blob type is known to the blob filesystem after the
        type 2 blobs have been written.

Following this scheme, it is guaranteed that after the migration every blob
on the device is a type 2 blob, including those whose raw content (and hence its
name) has not changed during the migration. Subsequently, accurate reasoning
about forthcoming OTA is possible.

### Blob rewriting mechanism for type 2 blobs

Prior to this RFC, properties such as compression settings are not readily
visible without actually inspecting a blob. A common component may see that a
blob with a given hash exists in blobfs/fxblob. However, the package cache
would be unable to determine whether an installed blob needs to be reformatted
solely based on its name. This is because the name is derived only from its
contents, not its format. Furthermore, due to separation of concerns, the
package cache, responsible for downloading a blob, _should not have to know and
care_ about semantic properties which are important for blobfs/fxfs. Thus, the
mechanism for updating the blobs during an OTA from the viewpoint of the
on-device package management stack looks like this (pseudo-code, conceptually):

```
fn do_ota() {
    ...
    for hash in list_of_expected_hashes {
        let blob = match get_blob_by_hash_from_blob_storage(hash) {
            Some(blob) => blob,
            _ => {
                let blob = fetch_blob_from_server(hash);
                write_blob_to_blob_storage(&blob);
                blob
            }
        };
    }
    ...
}
```
This RFC proposes to add an info method to return metadata about the blob,
including if it would accept an overwrite of the blob to update some details
beyond the verified contents. The resulting pseudo-code for the
above sequence will turn into the following, again conteptually:

```
fn do_ota() {
    ...
    for hash in list_of_expected_hashes {
        let blob = match get_blob_by_hash_from_blob_storage(hash) {
            (Some(blob), info) if info.desired_type => blob,
            _ => {
                let blob = fetch_blob_from_server(hash);
                write_blob_to_blob_storage(&blob);
                blob
            }
        };
    }
    ...
}
```

This has the following advantages:

*   Blobfs can expose a signal to the package management stack during an OTA
    which will cause the re-download of an existing blob in its new format by
    returning `false` for `desired_type`.
*   The separation of concerns between the blob filesystems and the package
    management stack is kept, i.e. it is not necessary for the package cache to
    know what type of blob it is dealing with.
*   Existing mechanisms in fxblob / blobfs can guarantee the atomicy of the
    write completion, i.e. it is ensured that the rewrite of the blob will not
    be interrupted halfway through the write and end up with a corrupted blob.

It is worth noting that this RFC, as illustrated in above pseudo-code proposes
to perform the migration from type 1 to type 2 delivery blobs via _offline
compression_, i.e. the new blobs for products are made available server side.
Subsequently, the new blobs are written as part of an OTA. This proposal is made
under the observation that for said migration the trade-off between network
bandwidth and device performance across the known fleet of Fuchsia-based
products clearly points to a server side solution to generate the type 2 blobs.
For later changs this might not necessarily be the case, and is described below.

### Blob rewriting mechanism for later blob types

Chances are that in future additional blob types emerge, and existing products
may be migrated to these upcoming blob types. These migrations will be slightly
different from the type 1 to type 2 migration described above, because the type
1 to type 2 migration is the one where on-disk metadata for blob types is
initially introduced. This means that:

*   For the type 1 to type 2 migration, the blob storage layer can easily
    identify that a blob needs to be updated when possible by inspecting an
    existing blob's metadata: If there is none, it needs to be updated.
*   For later migrations, the determination will not be made based on the
    existence of metadata, but based on its content. The desired type will be
    determined by a build-time constant.
*   This allows to keep the interface between the on-device storage layer and
    the software delivery stack unchanged between the type 1 to type 2 migration
    versus later migrations: By returning `false` for the `desired_type` the
    storage layer will continue to be able to signal to the SWD stack that
    re-downloading of a blob is required.

As mentioned in the previous section, since [RFC-207] there is a distinction
between _offline_ and _online compression_ in the context of blobs. This is
from the viewpoint of a Fuchsia based product, where _online compression_ means
that the device downloads a blob and compresses it itself into the desired
format upon write. _Offline compression_ on the other hand refers to the scheme
that the blob is already served by the blob server in the desired format such
that the device can perform a simple write.

This RFC intentionally does not mandate the handling of future blob type
migrations to be either _online_ or _offline_. This will be determined depending
on the circumstances of the respective migration.

## Implementation {#implementation}

The implementation of this RFC is constrained by the stepping stones which need
to be established to guarantee OTAs of production devices work at all times.
Thus, the implementation can be conceptualized as three phases.

### First phase: Prior to stepping stone

*   Implementation of support for blob overwrite and updatable status in the
    blob storage layer (Local Storage).
*   Implementation of support in the package management stack for type 2
    delivery blobs (SWD).
*   Support for creation of product bundles with type 2 delivery blobs
    (Assembly).
    * The default setting is still type 1 at this point, until the switch to
      type 2 is performed in the second phase.
*   Support for serving type 2 delivery blobs (Infrastructure).
*   Support for exposing the status flag for the blob rewrite policy
    (Local Storage).

### Second phase: Stepping stone build

*   The stepping stone build will switch to type 2 delivery blobs (Assembly).
*   The server side infrastructure will serve both type 1 and type 2 delivery
    blobs to clients for the stepping stone build.
    *   The clients will download and install the type 1 blobs for the stepping
        stone build, since code that implements the switch to downloading type 2
        blobs by default is part of this stepping stone build, see the bullet
        points below.
    *   The mechanics of serving both types will leverage the tooling as
        described in the section on [ergonomics](#ergonomics).
    *   The mechanism won't necessarily be made available to builds thereafter,
        because the package management stack on-device will start looking for
        type 2 blobs starting with the stepping stone build, see next bullet
        point.
*   Package management will download type 2 delivery blobs (SWD).
*   The blob storage layer will default to writing out the new desired format
    (Local Storage).

### Third phase: After stepping stone

*   The type 1 delivery blobs will no longer be part of the product bundles
    (Assembly) and thus won't upload any new ones while continuing to serve the
    existing ones (Infrastructure).

## Performance

The functionality as proposed by this RFC will have some performance impact, but
we expect this impact to be minor, and in part only temporary.

*   The update that moves blobs into their desired state in storage may take
    slightly longer than usual as there will be no blobs that are skipped during
    the update process as the ones existing from the previous update will be
    downloaded for overwriting.
*   The migration from type 1 to type 2 delivery blobs may introduce a change of
    the blob compression settings, which will result in a performance hit during
    assembly. At the time of writing an estimate cannot be given without
    specifying the exact compression parameters. This is outside the scope of
    this RFC and will be determined in a product specific scope.
    *   The type 2 blobs themselves are expected to have negligible
        performance impact on all known products during normal device operation.
        To ensure that the rollout will not impact the user experience, the
        performance metrics of the test fleet shall be monitored during
        feature development and testing of the migration.
*   The introduction of a new delivery blob type will introduce new tests to
    make sure the delivery blob type works. This is expected to be comparable
    performance-wise to the tests in existence for type 1. It may include some
    additional testing of edge cases, e.g. fallback to type 1 in the absence of
    type 2 and so on.

## Ergonomics {#ergonomics}

After [RFC-207], `ffx` has gained support for delivery blobs. As of writing this
RFC, this defaults to type 1 delivery blobs, since this is the latest version.

Following the acceptance of this RFC, we will extend all ffx tools and plugins
which currently handle type 1 blobs to be able to handle type 2 blobs as well.

*   For assembly, `ffx product` will use type 2 blobs by default.
    *   An `ffx` option to create type 1 blobs will be retained.
*   Turning existing type 1 into type 2 blobs will be handled by formatting
    the type 1 blob delivery blob into a non-delivery blob and formatting it
    into a type 2 delivery blob. The existing functionality in ffx will be
    extended to accomplish this. It will look conceptually like the following
    example:

```
# Compressing an uncompressed blob into v1
$ ffx package blob compress --blob-format v1 --output <output_file> uncompressed_input_file

# Compressing an uncompressed blob into v2
$ ffx package blob compress --blob-format v2 --output <output_file> uncompressed_input_file

# Recompressing a v1 blob into v2
$ ffx package blob decompress --output uncompressed_blob v1_compressed_input_file
$ ffx package blob compress --blob-format v2 --output <output_file> uncompressed_blob
```

Note that the subcommands' names `compress` and `decompress` are for historic
reasons and might change into something more general in the future. Also, the
`decompress` subcommand will be able to detect the blob format it finds, and
will not need explicit passing of a `--blob-format` argument.

The places that set type 1 as the default blob type will be bumped to type 2 to
produce the stepping stone build, leading to the following changes in
ergonomics:

*   When `ffx` takes a non-delivery blob and generates a delivery blob from it,
    it will default to type 2 unless specified otherwise.
*   For assembly, `ffx product` will default to generate type 2 blobs only,
    unless specified otherwise.
*   There will be switches in `ffx` to specify the intended delivery blob type.

## Backwards Compatibility

Changes to the on-disk blob format shall not interrupt the operation of
Fuchsia-based products or development workflows. Hence, the design and
implementation plan strives to ensure backwards compatibility.

### Production

In the same way the introduction of delivery blobs was handled during the
implementation of [RFC-207], the server providing updates for production devices
will serve type 1 and type 2 delivery blobs for the stepping stone build. This
ensures all devices can update through the stepping stone, regardless of whether
they request type 1 or type 2 blobs.

This approach allows us to begin rolling out new blob types as early as
development pace permits, and a stepping stone is only needed to end the
transition. During the transition, the client side will switch to using type 2
delivery blobs with the first build supporting it by adapting the requested URLs
accordingly.

For the format changes that introduce the change to producing v2 delivery blobs
only, a stepping stone is introduced. This guarantees that devices will not
update past a point where incompatibilities could arise.

### Development

The development host will switch from publishing type 1 to type 2 delivery blobs
with one change. In the same way as [RFC-207], the development host will support
only one blob format at a time. As such, the host side server is not guaranteed
to have older blob formats for backward compatibility.

Unlike production builds which go through a stepping stone, development
workflows do not have a mechanism which enforces that updates go through a
certain build. Hence, `fx ota` will depend on device side fallback, i.e.
fxblob/blobfs will perform the necessary actions depending on the incoming
blobs.

## Security considerations

This RFC does not plan to extend the attack surface beyond the scope explained
in [RFC-207], Since changing the on-disk format of blobs could be performed more
frequently without special preparations, it should be ensured that the
decompressor is continuously updated to make sure there is no known
vulnerability.

## Privacy considerations

There are no privacy relevant changes planned with this RFC. The delivery blob
type has been transferred between client and server since [RFC-207].

## Testing

### Automated testing and test code

In the process of implementing the offline compression and delivery blobs
features, as described in [RFC-207], unit tests, integration tests for the
package resolver and E2E OTA tests have been implemented. With the
implementation of type 2 delivery blobs, the currently existing set of tests, as
well as the on-device storage layer tests, are going to be extended to cover the
new cases. When designing and implementing the tests, particular focus on some
points must be taken into account:

*   Since only the type 1 delivery blob is in use at the time of this writing,
    tests and production code may make implicit assumptions about the delivery
    blobs. This will be adapted to cover type 2 blobs as well.
*   Some code may have to implement specific handling like fallbacks or
    converting type 1 into type 2.
*   Some code flows in the package resolver or the storage layer may be specific
    to type 1 or 2 blobs, intentionally not supporting the other type. These
    should be marked clearly, along with comments to state the rationale.
*   Changes to the blob format likely will not stop after type 2. Thus, every
    rewrite and extension of the current tests shall be rewritten in a
    generalized form to ensure minimal effort is necessary to introduce
    additional delivery blob type, by one of e.g.:
    *   automatically using the "latest" type,
    *   looping over each supported type, or
    *   being explicitly tied to a specific type, and deletable when that type
        is no longer supported.
*   Test code for tooling, e.g. `ffx` as well as build and assembly needs to be
    rewritten or added in the same fashion as stated above: Extensible to
    possible upcoming delivery blob types, provided rationale for non-obvious
    cases, specific to where they apply, etc.
*   There is always the chance for OTAs to be interrupted, for instance due to
    power outages. If this happens, a device might end up with parts of the
    blobs having updated to type 2 whereas others have not. The blob filesystems
    already have [tests for these scenarios in
    place](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/storage/blobfs/test/unit/blobfs_migration_test.cc;l=263;drc=3496ec5a8cbf5de43cfcd329b0c0cdf6e1130ee4).
    Where applicable, these tests will be extended as needed.

### Manual testing

Additionally, for products, the manual OTA tests conducted by the test team and
subsequent dogfood builds will verify the test results from the aforementioned
automated testing.

## Documentation

Upon acceptance of this RFC we will update the [documentation of
BlobFS][blobfs-desc] to include the new flags that will be used for tracking the
relevant state changes. The additional status checking request for in-tree
[BlobReader](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/storage/fxfs/fidl/fuchsia.fxfs/fxfs.fidl;l=260;drc=636b347ba89a57d1318fe905de2a19325fc22afe)
protocol and what it means for the `allow_existing` flag in
`BlobCreator::Create` will need to be detailed as well.

## Drawbacks, alternatives, and unknowns

### Drawbacks

The proposal comes with some drawbacks. They are rooted in the increased
complexity of the implementation compared to the status quo. However, the
increased complexity is benign, such that the benefits outweigh the drawbacks:

*   Storage of additional blob metadata to manage the transition creates a lot
    of state combinations, and those combinations cannot be cleaned up until a
    second stepping stone is done, which is intentionally not scheduled as part
    of this change to reduce the number of stepping stones.
*   As mentioned before, for the transition period both type 1 and 2 delivery
    blobs will be produced and stored on the blob server, requiring additional
    build time and disk space.

### Alternatives

*   For a single migration, it might be possible to implement a blob format
    change by building implicit assumptions into the package management stack
    and the on-device storage layer. But it would require that, in case another
    blob format migration is necessary in the future, another design with a
    similar scope would have to be written and implemented. With several
    migrations in succession, it would be tricky to handle edge cases properly.
    Hence the introduction of on-disk blob metadata storage is more sustainable.
*   For certain developer workflows, when working with TUF repositories (i.e.
    non-production devices), there might be edge cases where developers need to
    `ffx flash` their devices after an unsuccessful `fx ota`. Most if not all of
    those cases could be resolved by providing both type 1 and 2 delivery blobs
    in the same product bundle. This was considered in the lead-up to this RFC,
    but due to the considerable amount of additional build time and extra space
    this would occupy in the product bundle, ultimately decided against.

#### Alternatives to stepping stones

When considering introducing stepping stones, a question always is the cost vs.
return benefit. Generally, stepping stones are to be avoided if possible, since
they increase the number of updates a device on an older software version has to
go through until it is up-to-date. Also it takes longer, and requires
additional server-side bandwidth.

Stepping stones are required if an older version A needs to ensure that a
property is satisfied that a newer version B requires to hold true. For
instance, if version B removes the ability to deal with a certain blob type,
then it must be guaranteed that a prior version of the software updates the
blobs to the desired format before rebooting into version B. The prior version
must become the _stepping stone_.

In this RFC, it is proposed to introduce a stepping stone build to facilitate
the replacement of the type 1 with the type 2 blobs. The pros and cons for this
approach are:

* Pros:
  * It is possible to clearly, unambiguously reason about OTA size requirements
    of storage constrained devices: After the type 2 blobs have been written and
    the device reboots into the new version, the old blobs can be garbage
    collected.
  * The server-side infrastructure can stop producing type 1 blobs after the
    stepping stone build since it is guaranteed that following builds will
    request type 2 blobs from the correct URLs.
* Cons:
  * The introduction of the stepping stone means that every device has to go
    through this particular update, increasing the number of OTAs a device with
    an old version (e.g. a device just purchased in a store) has to take until
    it is up-to-date.
  * All blobs will be downloaded, even if their uncompressed content is
    identical with a type 1 blob that is still in blobfs from the previous
    version.

Technical alternatives to stepping stones also come with their own pros and
cons. For the migration of type 1 to type 2 stepping stones, specifically,
avoiding the stepping stone would mean:

* Pros:
  * Not taking a stepping stone means a device can update straight to a newer
    build without going through the stepping stone.
  * The expected peak of the number of overall downloads from the server
    infrastructure resulting from the migration is less pronounced.
* Cons:
  * Reasoning about the OTA storage requirements becomes difficult, since many
    more past verssions may seek an update to an up-tp-date version. This may
    result in (old version, new version) tuples which exceed the available
    storage on a device's blob storage layer.
  * The server side infrastructure would have to continue serving type 1 blobs
    for newer builds, since older devices that are not aware of type 2 would
    continue to request type 1 blobs from the update server until they have
    updated to a version new enough to support type 2.

Other, more sophisticated OTA schemes are conceivable to address some of the
shortcomings of the previous solutions. For example, a way to migrate the blob
storage without booting into a full system could be devised, either via the
recovery mode or via a special update mode, both of which have not been
implemented at the time of writing this RFC.

* Pros:
  * No stepping stone but still ability to reason about the size requirements
    for the blob migration from type 1 to type 2, and later migrations.
  * No expected download peak on the server side due to the lack of a stepping
    stone for all devices.
* Cons:
  * No implementation for these mechanisms currently exist. They are non-trivial
    and might require a considerable amount of resources to implement, possibly
    more then this RFC.
  * Handling fallbacks and unforeseen edge cases correctly may cause additional
    work, including the additional testing.

Summarising, while there may be ways to circumvent the creation of a stepping
stone, this RFC does not propose any of them for this particular migration, and
instead proposes to leverage the well understood stepping stone procedure for
the migration of type 1 to type 2 blobs.

## Future work

### Non-integer-esque blob types

This RFC deliberately avoids referring to blob type as _versions_, even though
it might be tempting to think of them as such, in particular because only type 1
and type 2 are discussed in this RFC. However, the blob type is deliberately an
[enum](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/storage/lib/delivery_blob/src/lib.rs;l=205-210;drc=581020cd75acea43b959717316efbf42211fbd21),
not an integer, and no particular ordering is assumed. The introduction of type
2 does **not** mean type 1 is automatically superseded.

In the future, we might assign more descriptive names to blob types. This might
make it more obvious to instantly understand from their name what they are and
why they were introduced, rather than requiring to look it up in the code
comments or commit history.

### Client-server communication on blob types

At the time of writing this RFC, only type 1 delivery blobs exist. Furthermore,
the above described update mechanism allows to perform updates of client devices
to type 2 without explicit communication between the blob server and the client
regarding the blob type.

In the future, when additional blob types become available, we might consider
a scheme where the client can inform the update server about what blob types it
prefers or could accept. While this is deliberately out of scope for this RFC,
a potential approach is for the client to send this information in the HTTP
request when downloading blobs. This would allow the blob server to send the
most suitable blob type in its response.

### Leveraging anchored packages

In [RFC-271], the package set of _anchored packages_ is described. Among others,
this allows _pinning packages_ without downloading and committing them to the
blob filesystem. An OTA can make packages known to the target at the time of
installing the update package, but instead of downloading the blobs prior to
rebooting into the new version, the packages are fetched upon booting into the
new version. This could potentially be leveraged to facilitate the migration to
a new blob type.

## Prior art and references

* [Anchored packages RFC][RFC-271]
* [Blob compression RFC][RFC-207]
* [BlobFS description on the Fuchsia development site][blobfs-desc]

<!-- Links -->

[RFC-271]: /docs/contribute/governance/rfcs/0271_anchored_packages.md
[RFC-207]: /docs/contribute/governance/rfcs/0207_offline_blob_compression.md
[blobfs-desc]: /docs/concepts/filesystems/blobfs.md