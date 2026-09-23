# Packages

A package is the unit of installation on a Fuchsia system. This document describes
various workflows for building and installng a package.

Note: The majority of these workflows rely on the `ffx` tool which is available
in `//tools`.

The workflows are:

* [Build a package](#build-package)
* [Publish a package](#publish-package)
* [Install a package](#install-package)
* [Run a component from an installed package](#run-component)

For more details, see the help messages from `ffx package build help`,
and `ffx repository publish help`.

## Build a package {#build-package}

To build a package:

1. Create a `meta` directory:

   ```posix-terminal
   mkdir -p {{ '<var>' }}PACKAGE_DIR{{ '</var>' }}/meta
   ```

   Replace <var>PACKAGE_DIR</var> with the staging directory where the package
   is going to be built.

1. Set the `$META_PACKAGE_FILE` environment variable:

   ```posix-terminal
   export META_PACKAGE_FILE={{ '<var>' }}PACKAGE_DIR{{ '</var>' }}/meta/package
   ```

1. Open a text editor and create the `$META_PACKAGE_FILE` file with
   the following content:

   ```none
   {
     "name": "<intended name of your package here>",
     "version": "0"
   }
   ```

   The version number is currently required to be `0`.

1. Save the file and close the text editor.

1. Create a [package build manifest file][build-manifest-file] (`$BUILD_MANIFEST_FILE`),
   which provides the paths to all the package content files, and export its
   path via the variable:

   ```posix-terminal
   export BUILD_MANIFEST_FILE={{ '<var>' }}BUILD_MANIFEST_FILE{{ '</var>' }}
   ```

   Each line of a manifest file maps to a file contained in the package and
   is in the form of `destination=source` where:

   * `destination` is the path to the file in the final package.
   * `source` is the path to the file on the host machine.

   The manifest file must include at least one line for the package ID file,
   for example:

   ```none {:.devsite-disable-click-to-copy}
   meta/package=/path/to/meta/package
   ```

   Additional files to be added to the package must be listed in the build
   manifest file in the same way.

1. Go to the <var>PACKAGE_DIR</var> directory:

   ```posix-terminal
   cd {{ '<var>' }}PACKAGE_DIR{{ '</var>' }}
   ```

1. Generate a package manifest file, which creates the package metadata archive
   at <var>PACKAGE_DIR</var>`/meta.far`:

   ```posix-terminal
   ffx package build $BUILD_MANIFEST_FILE -o {{ '<var>' }}PACKAGE_DIR{{ '</var>' }} --api-level HEAD
   ```

   This command creates the package manifest file implicitly as
   {{ '<var>' }}PACKAGE_DIR{{ '</var>' }}`/package_manifest.json`.

   Note that the `--api-level` is preferably used to target a specific API
   level, given by a numeric argument. In this example we use HEAD, i.e. the
   latest in-development API.

1. Set the `$PACKAGE_MANIFEST_FILE` environment variable:

   ```posix-terminal
   export PACKAGE_MANIFEST_FILE="{{ '<var>' }}PACKAGE_DIR{{ '</var>' }}/package_manifest.json"
   ```

   If the contents of the package change, you need to re-run the
   `ffx package build $BUILD_MANIFEST_FILE` command.

1. Create a package archive, which gathers all the package contents into
   a single distributable file:

   ```posix-terminal
   ffx package archive create -o "{{ '<var>' }}PACKAGE_DIR{{ '</var>' }}/{{ '<var>' }}PACKAGE_NAME{{ '</var>' }}.far" "$PACKAGE_MANIFEST_FILE"
   ```

   Replace <var>PACKAGE_NAME</var> with the intended name of the package.

   This command creates the package archive as <var>PACKAGE_NAME</var>`.far`.

1. Set the`$PACKAGE_ARCHIVE` environment variable:

   ```posix-terminal
   export PACKAGE_ARCHIVE={{ '<var>' }}PACKAGE_DIR{{ '</var>' }}/{{ '<var>' }}PACKAGE_NAME{{ '</var>' }}.far
   ```

   If the contents of the package change, you need to re-run the
   `ffx package build` and `ffx package archive create` commands.

You have successfully built a package. Now you are ready to publish the package.

## Publish a package {#publish-package}

Note: The workflow in this section uses the environment variables set in
the previous [Build a package](#build-package) section.

To publish a package:

1. Initialize a directory that serves as a packages repository:

   ```posix-terminal
   ffx repository create {{ '<var>' }}REPO{{ '</var>' }}
   ```

   This creates a directory structure under the directory <var>REPO</var> which
   is ready for publishing packages.

1. Publish package manifests to the repository:

   ```posix-terminal
   ffx repository publish --package $PACKAGE_MANIFEST_FILE {{ '<var>' }}REPO{{ '</var>' }}
   ```

   `ffx repository publish` parses `$PACKAGE_MANIFEST_FILE` and publishes the
   package in the provided <var>REPO</var> directory.

   The `--package` argument can be repeated. If you run this command multiple
   times with different package manifests, each instance will be published to
   the same repository. New versions of the same packages can be published using
   the same command.

1. (Optional) Publish package archives to the repository:

   ```posix-terminal
   ffx repository publish --package-archive $PACKAGE_ARCHIVE {{ '<var>' }}REPO{{ '</var>' }}
   ```

   `ffx repository publish` parses `$PACKAGE_ARCHIVE` and publishes the
   package in the provided <var>REPO</var> directory.

   The `--package-archive` argument can be repeated. If you run this command
   multiple times with different package archives, each instance will be
   published to the same repository. New versions of the same packages can be
   published using the same command.

You have successfully published a package. You are now ready to install a
package.

## Install a package {#install-package}

To install a package:

1. Start the package server and serve the repository to the target:

   ```posix-terminal
   ffx repository serve --repository "{{ '<var>' }}REPO_NAME{{ '</var>' }}" --repo-path "{{ '<var>' }}REPO{{ '</var>' }}"
   ```

   By default, this starts a repository server on the host machine, listening on
   port `8083`. This introduces the repository to the target as an update
   source. The `--repository "{{ '<var>' }}REPO_NAME{{ '</var>' }}"` is optional, but useful.

1. (On the target device) Check the configured repository:

   ```
   pkgctl repo -v
   ```

   You should see a repository configured, listing (among other configuration
   variables) its repository url `"repo_url": "fuchsia-pkg://<REPO_NAME>"`.

1. Download the package:

   ```posix-terminal
   ffx target package resolve fuchsia-pkg://{{ '<var>' }}REPO_NAME{{ '</var>' }}/{{ '<var>' }}PACKAGE_NAME{{ '</var>' }}
   ```

   If the component is not already present on the system, the package resolver
   on the target device downloads the package and places the blobs in the
   blobFS. If the package already exists but a newer package is available on the
   package server, the updates will be downloaded.

   Note: If you work on an older version of Fuchsia, `ffx target package` might
   not be available, and the above command will error out. If this is the case,
   run the following command **on the target device shell** as a fallback:
   `pkgctl resolve fuchsia-pkg://{{ '<var>' }}REPO_NAME{{ '</var>' }}/{{ '<var>' }}PACKAGE_NAME{{ '</var>' }}`

You have successfully installed or updated the package. You are now ready to
run a component from the installed package.

## Run a component from an installed package {#run-component}

To run the component, use the `ffx component run` tool. For help: `ffx component run --help`.

For the `url` parameter provide a URL in the form of
`fuchsia-pkg://<REPO_NAME>/<PACKAGE_NAME>#meta/<COMPONENT_NAME>.cm`.

<!-- Reference links -->

[build-manifest-file]: /docs/development/build/gn_concepts/internals/manifest_formats.md
