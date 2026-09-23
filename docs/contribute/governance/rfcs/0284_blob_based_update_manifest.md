<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0284" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

The current [Fuchsia OTA process] relies on an [update package] representing the
manifest of a system update with its `packages.json` and `images.json` member
files pointing to Fuchsia system packages and Partition images respectively,
themselves organized as Fuchsia packages.
This design tightly couples the update process to the ephemeral package
resolution system. While this reuses existing machinery, it introduces several
limitations:

1. **Complexity**: Instead of fetching blobs directly, the `system-updater`
   component relies on the package resolver to fetch the content of the resolved
   package. This requires the package resolver to maintain a complex chain of
   trust and metadata, parsing meta.far files, and recursively resolving
   subpackages, just to determine the list of blob hashes.
1. **Flexibility**: Because the current update process is package-based, to
   fetch a blob, it must be part of a package. This necessitates the creation
   of fake packages just to fetch additional blobs needed for backward
   compatibility. Furthermore, this does not allow skipping fetching blobs that
   are not needed, for example if the blob is already available in bootfs.
1. **User Experience**: One of the common current problems with package-based
   updates is that the update progress is based on the number of packages
   resolved. In some products, there is one package that is much larger than
   others, but `system-updater` does not know the size of packages, only the
   number of packages. This leads to the progress bar appearing to be stuck at
   99%, while it is just resolving that one big package. With blob-based
   updates, progress will be based on the number of blobs fetched, where each
   blob has a weight of its uncompressed blob size, leading to a more accurate
   and more responsive progress bar.

## Summary

This RFC introduces a new blob-based OTA manifest, as an alternative to the
package-based contents of the [update package]. It allows system-updater to
fetch blobs directly instead of resolving [packages][package] which implicitly
fetches the package's blobs.

## Stakeholders

_Facilitator:_

- hjfreyer@google.com

_Reviewers:_

- alizhang@google.com (Security)
- awolter@google.com (Software Assembly)
- etryzelaar@google.com (SWD)
- gtsai@google.com (Server Infrastructure)

_Socialization:_ This design has been socialized within the Software Delivery
and security teams.

## Requirements

The new update system must:

- Support all existing update capabilities (images, epoch, etc.).
- Have minimal metadata overhead.
- Not regress from the current software update security posture.
- Provide more accurate progress reporting.

## Design

The core proposal is to add platform support for a new blob-based system update
manifest.

In the [current architecture][Fuchsia OTA process], `system-updater` relies on
the package resolver to fetch the packages listed in `packages.json`. This
obscures the actual content of the update behind the package resolution
process.

With the new blob-based manifest, we use a Protobuf-based [`OtaManifest`]
that explicitly lists every blob required for the update.
The `system-updater` parses this manifest and orchestrates the fetch of each
blob directly, bypassing the complex package resolution logic.

### Partition Image Updates

In the package-based update, partition images (like the ZBI or firmware
files) are bundled into separate packages themselves and listed in `images.json`
of the [update package]. This was done in [RFC-0170] to keep the main
[update package] small and avoid resolving large image blobs unnecessarily.
However, it requires the `system-updater` to actively resolve and cache
separate images packages before writing the images to the device's
partitions using `fuchsia.paver`. If any one of the images is needed, the
package it belongs to must be resolved, which triggers the fetch of *all* images
in that package, regardless of whether they are needed.

With blob-based update manifest, this process is significantly simplified. The
manifest directly lists the images and their corresponding blob parameters. The
`system-updater` can granularly fetch the exact image blobs it needs, verify and
then write them to the partition, and discard them, without dealing with
intermediate packages.

### Manifest Format

The `OtaManifest` is a **Protobuf** format. Protobuf offers a compact binary
format, strict schema enforcement, and forward/backward compatibility.

**Schema:**

```protobuf
syntax = "proto3";

package fuchsia.update.manifest;

// The root message for an Over-The-Air (OTA) update manifest.
message OtaManifest {
  // The version from the `build-info` of the target build. This field is for
  // informational purposes only and does not change the updater's behavior.
  string build_info_version = 1;

  // The board this OTA is for (e.g., "x64", "arm64"). The system updater will
  // reject the OTA if this does not match the device's expected board name
  // from `build-info`.
  string board = 2;

  // The epoch of this OTA. See RFC-0071 for details.
  uint64 epoch = 3;

  // The update mode, indicating if this is a normal update or a forced
  // recovery.
  UpdateMode mode = 4;

  // The base URL prefix of the blobs, including the delivery blob type. The
  // final URL for each blob will be "{blob_base_url}/{fuchsia_merkle_root}".
  // Relative URLs are supported, and will be resolved relative to the URL of
  // the OTA manifest.
  string blob_base_url = 5;

  // The partition images that should be written during the update.
  repeated Image images = 6;

  // Additional blobs that should be written to blob storage.
  repeated Blob blobs = 7;
}

// The mode of the update.
enum UpdateMode {
  // A standard system update.
  NORMAL = 0;

  // An update that forces the device into recovery mode.
  FORCE_RECOVERY = 1;
}

// The target slot for an image.
enum Slot {
  // The primary A/B slot.
  AB = 0;

  // The recovery slot.
  R = 1;
}

// An image to be written to a partition.
message Image {
  // The type of the image.
  oneof image_type {
    // A standard system asset like ZBI or VBMETA.
    AssetType asset = 1;

    // A firmware image, with the field value specifying the board specific
    // firmware type, which will be passed to paver verbatim. If paver doesn't
    // support the firmware type, the firmware image will be skipped.
    string firmware = 2;
  }

  // The slot this image should be written to.
  Slot slot = 3;

  // Metadata about the blob containing the image data.
  Blob blob = 4;
}

// The type of a system asset.
enum AssetType {
  // A Zircon Boot Image.
  ZBI = 0;

  // Verified Boot Metadata.
  VBMETA = 1;
}

// Metadata for a blob.
message Blob {
  // The fuchsia merkle root of the uncompressed blob data.
  bytes fuchsia_merkle_root = 1;

  // The uncompressed size of the blob in bytes.
  uint64 uncompressed_size = 2;
}
```

### Incremental OTA Support

The switch from package-based updates to blob-based updates enables incremental
OTA support because `system-updater` will update each blob individually. In the
current package-based system, the resolver manages blob fetching, which obscures
the relationship between the target blob and any existing version on the device.
By listing blobs directly in the manifest, `system-updater` can manage the
update of each blob, applying patches where applicable.

Extra fields could be added to the [`OtaManifest`] to support incremental
updates, allowing us to explore the delta methods described in [RFC-0207]. The
detailed design for incremental OTAs will be provided in a future RFC.

### Tooling

An `ffx` subcommand will be added to inspect the [`OtaManifest`].
This tool will allow developers to view the manifest's headers and pretty-print
the decoded Protobuf payload for debugging.

## Implementation

1. **Productionize Prototype**: A blob-based manifest prototype is already
   implemented in `system-updater`, but needs to be productionized and match the
   final design in this RFC.
1. **Build Rules**: Modify the build rules for producing product bundles to
   generate the new Protobuf manifest.
1. **Infrastructure Integration**: Update infrastructure that creates the
   [update package] to also support generating and serving the new manifest
   format.
1. **`fx ota` Support**: Update `fx ota` to support serving and triggering
   updates using [`OtaManifest`].

## Performance

- **Granular Fetching**: Images are fetched individually only when needed.
- **Streamlined Fetching**: All content blobs are known immediately after
  parsing the manifest, allowing fetching to proceed without waiting for
  intermediate `meta.far` files to be fetched and parsed.
- **Granular Reporting**: Progress can be reported per-blob immediately, rather
  than waiting for package resolution steps.
- **Granular Suspend/Resume**: The update can be suspended and resumed at the
  level of individual blobs.

## Ergonomics

This proposal simplifies the mental model of system updates for developers and
release engineers. Instead of reasoning about complex package resolution rules,
ephemeral versus base packages, and `meta.far` contents, updates are presented
as a flat, explicit list of blobs and partition images. This makes debugging OTA
failures significantly easier, as the state of the update is directly visible
from the manifest and the set of downloaded blobs.

The addition of the `ffx` subcommand to inspect the [`OtaManifest`] will
provide a straightforward way for developers to verify the contents of an update
during the build and release process.

## Backwards Compatibility

The switch to Protobuf for the manifest ensures that we can evolve the schema
while maintaining backward compatibility. New fields can be added to the
[`OtaManifest`] message without breaking existing clients.

A device can transition from package-based to blob-based updates by first
updating to a build that supports blob-based updates using the existing
package-based update mechanism.

## Security considerations

Because Fuchsia is not enforcing any package- or image- or firmware-specific
checks, removing those package boundaries does not regress from the current
implemented security posture for Fuchsia's software updates.

The build-time process of producing a blob-based manifest from all package
targets should have tests and tooling that help ensure the translation is
idempotent and auditable.

## Privacy considerations

The proposed blob-based manifest format only removes the package resolution
indirections from the current packages.json and images.json based manifest --
essentially moving from runtime package resolution to build time resolution.

The removed package boundaries are not utilized in the current implementation.

The semantics of how Fuchsia manages software delivery remain largely unchanged,
thus there is no regression or improvement in privacy considerations.

## Testing

The support for blob-based manifest will be validated using existing test
frameworks across unit, integration, and end-to-end (E2E) tiers:

- **Unit Tests:** Expand `system-updater` and `update-package` tests to cover
  [`OtaManifest`] parsing.
- **Integration Tests:** Create blob-based variants for all existing
  `system-updater` integration tests.
- **End-to-End (E2E) Tests:** Extend existing E2E OTA tests to validate
  blob-based updates.


## Documentation

Update existing fuchsia.dev documentation to describe the new manifest format
and mention the `ffx` subcommand that can inspect the manifest.

## Drawbacks, alternatives, and unknowns

### Drawback: Missing Blobs in Manifest

Because the OTA process is no longer explicitly package-based, it relies
entirely on the manifest correctly identifying and listing every individual blob
required by the system. This introduces an increased risk: a bug in the manifest
generation tooling could produce a manifest that inadvertently omits blobs
needed by base packages.

If these missing blobs are not accessed during the initial system boot
sequence, the device might successfully boot. Without intervention, it would
then commit the OTA, and the missing blobs would only be discovered later at
runtime, leading to crashes or failures after the update cannot be easily
rolled back.

To mitigate this risk, we will implement an OTA health check directly in
`pkg-cache` that verifies all blobs required by the base packages are physically
present on the device. The `system-update-committer` component queries the
system's overall health (via `fuchsia.update.verify.HealthVerification`) before
committing an OTA. By exposing this check from `pkg-cache` and including it
in the system's health verification suite, we can ensure we do not commit an
update from a buggy manifest that left out blobs.


### Alternative: JSON Manifest

We considered using JSON. While human-readable, it is verbose and lacks the
strict schema evolution capabilities of Protobuf. Protobuf is chosen for wire
efficiency and type safety.

### Alternative: Persistent FIDL

We considered using Persistent FIDL (as defined in [RFC-0120]) as the
serialization format for the manifest. Since FIDL is Fuchsia's native interface
definition language, using persistent FIDL would align with Fuchsia's platform
conventions.

However, the FIDL wire format prioritizes decoding speed, which introduces
substantial padding (elements are aligned to 8-byte boundaries) and structural
overhead (e.g., 16-byte envelopes for each table field, and vector headers).
Because the manifest will be transferred over the network, size is more
important than decode speed. While it is possible to compress the Persistent
FIDL to get similar sizes to Protobuf, this adds an additional layer of
complexity.

### Alternative: Columnar Store Format

We considered using a "columnar" approach for defining the blob list within the
Protobuf manifest. Instead of a single `repeated Blob blobs` field containing an
array of structs, this approach would use parallel arrays (e.g.,
`repeated uint64 blob_uncompressed_sizes` and
`repeated bytes blob_fuchsia_merkle_roots`).

While a columnar layout can be slightly more compact on the wire and faster to
iterate over a single attribute (such as extracting only the hashes for
verification), the actual size difference is negligible due to Protobuf's
already compact wire format. Furthermore, parallel arrays do not map cleanly to
image blobs, which require additional structured metadata. Ultimately, a
columnar layout significantly degrades the ergonomics of the data structure.

Grouping a blob's attributes together in a single `Blob` message (an "array of
structs" approach) provides better logical cohesion, simplifies the code that
processes the manifest, and makes it easier to add new optional fields for each
blob in the future.

## Prior art and references

- **Android/ChromeOS Update Engine**: This design is heavily inspired by
  Android's `payload.bin` [format][update_engine_proto] and `update_engine`,
  which uses a similar manifest approach.


[RFC-0120]: /docs/contribute/governance/rfcs/0120_standalone_use_of_fidl_wire_format.md
[RFC-0170]: /docs/contribute/governance/rfcs/0170_remove_binary_images_from_the_update_package.md
[RFC-0207]: /docs/contribute/governance/rfcs/0207_offline_blob_compression.md
[update_engine_proto]: https://android.googlesource.com/platform/system/update_engine/+/main/update_metadata.proto
[`OtaManifest`]: #manifest_format
[Fuchsia OTA process]: /docs/concepts/packages/ota.md
[package]: /docs/concepts/packages/package.md
[update package]: /docs/concepts/packages/update_pkg.md
