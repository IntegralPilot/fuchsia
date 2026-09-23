# Copyright 2024 The Fuchsia Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.
"""Python class for managing the copying of packages

Copies packages into a single directory structure, deduplicating the blobs into a single, combined
blobstore.
"""

import os

from serialization import json_dump, json_load

from .common import FileEntry, FilePath, fast_copy
from .package_manifest import BlobEntry, PackageManifest, SubpackageEntry

DepSet = set[FilePath]
Merkle = str
BlobSources = dict[Merkle, FilePath]

__all__ = [
    "PackageCopier",
]


class DuplicatePackageException(Exception):
    """To be raised when an attempt is made to add multiple packages with the same name to the same
    invocation of the PackageCopier"""

    ...


class PackageManifestParsingException(Exception):
    """To be raised when an attempt to parse a json file into a PackageManifest object fails"""

    ...


class PackageCopier:
    """Class for managing the copying of packages into a directory with a shared, combined, blobstore.

      This creates the following directory structure under the destination outdir:

      file layout:

            ./
          packages/
              <package name>
          blobs/
              <merkle>
          subpackages/
              <merkle>

    Files matching the patterns `packages/*/<package name>` and
    `subpackages/<merkle>` are JSON representations of the
    `PackageManifest` type (see `package_manifest.py`). (The `<merkle>` in
    `subpackages/` is the merkle of the subpackage's metafar.) A named package
    that is also referenced as a subpackage will have a copy of its manifest in
    both directories. (This is an intentional duplication to allow the
    simplification of the creation and use of the AIBs.)

    Each `PackageManifest` contains a `blobs` list of `BlobEntry` and an
    optional `subpackages` list of `SubpackageEntry`. The `source_path` of a
    `BlobEntry` references a file containing the blob bytes, in the `blobs/`
    directory. The `manifest_path` of a `SubpackageEntry` references another
    `PackageManifest` file in `subpackages/<merkle>`.
    """

    def __init__(self, outdir: FilePath) -> None:
        # The directory to create the AIB in.  The manifest will be placed in
        # the root of this dir.
        self.outdir = outdir

        # The internal paths, computed once for consistency
        self.packages_dir = os.path.join(outdir, "packages")
        self.subpackages_dir = os.path.join(outdir, "subpackages")
        self.blobstore = os.path.join(outdir, "blobs")

        # The package manifests:
        self.manifests: dict[str, tuple[FileEntry, PackageManifest]] = {}

        # The subpackages of each package
        self.subpackages: dict[Merkle, tuple[FileEntry, PackageManifest]] = {}

        # The files opened by the package copier
        self.deps: DepSet = set()

    def add_package(
        self, package_manifest_path: FilePath
    ) -> tuple[FilePath, PackageManifest]:
        """Add a package manifest to the set of packages to copy.

        Packages may not be fully copied until the 'perform_copy()' function is called.
        """
        # Open and parse the package manifest.
        self.deps.add(package_manifest_path)
        if not isinstance(package_manifest_path, str):
            raise ValueError(f"Found PackageDetails! {package_manifest_path}")
        manifest = open_package(package_manifest_path)

        package_name = manifest.package.name

        if package_name in self.manifests:
            raise DuplicatePackageException(
                f"A package with the name '{package_name}' already exists."
            )

        # Cache the destination path, and the manifest itself to use when copying
        paths = FileEntry(
            package_manifest_path, os.path.join(self.packages_dir, package_name)
        )
        self.manifests[package_name] = (paths, manifest)

        # Recursively find subpackages and add them to the set of packages to
        self._add_subpackages(os.path.dirname(package_manifest_path), manifest)

        # Return the future destination path, relative to the outdir, and the manifest itself, to
        # the caller in case they need the path for their own manifests, or have further validation
        # that they wish to perform on the data within the package manifest.
        rebased_destination = os.path.relpath(paths.destination, self.outdir)
        return (rebased_destination, manifest)

    def _add_subpackages(
        self, package_manifest_dir: FilePath, manifest: PackageManifest
    ) -> None:
        for subpackage in manifest.subpackages:
            if subpackage.merkle not in self.subpackages:
                # It's a new subpackage, so process it

                # Make the path relative to the package manifest if necessary.
                subpackage_manifest_path = subpackage.manifest_path
                if manifest.blob_sources_relative == "file":
                    subpackage_manifest_path = os.path.join(
                        package_manifest_dir, subpackage_manifest_path
                    )

                # Parse the package manifest.
                self.deps.add(subpackage_manifest_path)
                subpackage_manifest = open_package(subpackage_manifest_path)

                # Add it to the set of subpackages to copy.
                subpackage_destination = os.path.join(
                    self.subpackages_dir, subpackage.merkle
                )
                self.subpackages[subpackage.merkle] = (
                    FileEntry(subpackage_manifest_path, subpackage_destination),
                    subpackage_manifest,
                )

                # And recurse into its own subpackages.
                self._add_subpackages(
                    os.path.dirname(subpackage_manifest_path),
                    subpackage_manifest,
                )

    def perform_copy(self) -> tuple[list[FilePath], DepSet]:
        """Finalize the copy operation.

        All actual copying may be deferred until this function is called.
        """

        # The blobs to copy
        blobs: BlobSources = {}

        # The blob destination paths (relative to the outdir)
        blob_paths: list[FilePath] = []

        if self.manifests:
            os.makedirs(self.packages_dir)
            for paths, manifest in self.manifests.values():
                (package_blobs, package_deps) = _copy_package(
                    paths, manifest, self.subpackages_dir, self.blobstore
                )
                blobs.update(package_blobs)
                self.deps.update(package_deps)

        # For historical reasons, the subpackages dir is always created, but the other two folders
        # are only created when they have contents.
        os.makedirs(self.subpackages_dir)
        for paths, manifest in self.subpackages.values():
            (package_blobs, package_deps) = _copy_package(
                paths, manifest, self.subpackages_dir, self.blobstore
            )
            blobs.update(package_blobs)
            self.deps.update(package_deps)

        if blobs:
            # Copy all blobs
            os.makedirs(self.blobstore)
            for merkle, source in blobs.items():
                blob_destination = os.path.join(self.blobstore, merkle)
                blob_paths.append(
                    os.path.relpath(blob_destination, self.outdir)
                )
                self.deps.add(fast_copy(source, blob_destination))

        return (blob_paths, self.deps)


def open_package(package_manifest_path: FilePath) -> PackageManifest:
    with open(package_manifest_path, "r") as file:
        try:
            manifest = json_load(PackageManifest, file)
        except Exception as exc:
            raise PackageManifestParsingException(
                f"loading PackageManifest from {package_manifest_path}"
            ) from exc
    return manifest


def _copy_package(
    manifest_paths: FileEntry,
    manifest: PackageManifest,
    subpackages_dir: FilePath,
    blobstore: FilePath,
) -> tuple[BlobSources, DepSet]:
    """Copy a package manifest to the assembly bundle outdir, adding its
    blobs to the set of blobs that need to be copied as well. If the package
    has subpackages, recursively copy those as well, skipping any
    subpackages that have already been copied.
    """

    blobs: BlobSources = {}
    deps: DepSet = set()

    # Instead of copying the package manifest itself, the contents of the
    # manifest needs to be rewritten to reflect the new location of the
    # blobs within it.
    new_manifest = PackageManifest(manifest.package, [])
    new_manifest.repository = manifest.repository
    new_manifest.abi_revision = manifest.abi_revision
    new_manifest.set_paths_relative(True)

    # For each blob in the manifest:
    #  1) add it to set of all blobs
    #  2) add it to the PackageManifest that will be written, using the correct source path for
    #     internal blobstore.
    for blob in manifest.blobs:
        source = blob.source_path
        if source is None:
            raise ValueError(f"Found a blob with no source path: {blob.path}")

        # Make the path relative to the package manifest if necessary.
        if manifest.blob_sources_relative == "file":
            source = os.path.join(
                os.path.dirname(manifest_paths.source), source
            )

        blobs[blob.merkle] = source

        blob_destination = os.path.join(blobstore, blob.merkle)
        relative_blob_destination = os.path.relpath(
            blob_destination, os.path.dirname(manifest_paths.destination)
        )
        new_manifest.blobs.append(
            BlobEntry(
                blob.path,
                blob.merkle,
                blob.size,
                source_path=relative_blob_destination,
            )
        )

    for subpackage in manifest.subpackages:
        subpackage_destination = os.path.join(
            subpackages_dir, subpackage.merkle
        )
        relative_subpackage_destination = os.path.relpath(
            subpackage_destination, os.path.dirname(manifest_paths.destination)
        )
        new_manifest.subpackages.append(
            SubpackageEntry(
                subpackage.name,
                subpackage.merkle,
                manifest_path=relative_subpackage_destination,
            )
        )

    with open(manifest_paths.destination, "w") as new_manifest_file:
        json_dump(new_manifest, new_manifest_file)

    return (blobs, deps)
