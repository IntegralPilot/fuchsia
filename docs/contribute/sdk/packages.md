# Contribute SDK packages

This page serves as a playbook for contributing [Fuchsia packages][packages]
(from here on simply "packages") to the [Fuchsia API surface][api-surface]
and the [Fuchsia IDK][idk].

The steps are:

1. [Prerequisites](#prerequisites).
1. [Create a package API surface](#create-a-package-api-surface).
1. [Explore package contents](#explore-package-contents).
1. [Contribute to an API in the SDK](#contribute-to-an-api-in-the-sdk).

## Prerequisites {:.numbered}

For a package to be included in the Fuchsia SDK, the package must be created
in the category of `partner` or `public`. (For more information, see
[Promoting an API to the partner category][promoting-api].)

## Create a package API surface {:#create-a-package-api-surface .numbered}

The package API surface is created by populating two parameters in the
[`sdk_fuchsia_package()`][sdk-fuchsia-package-gni] target: `expected_files_present`
and `expected_files_exact`.

You can use these two parameters to enforce the following:

- `expected_files_present`: A package file is present in the SDK package's
  manifest.
- `expected_files_exact`: A package file is present in the SDK package's
  manifest and its content hash matches the content hash checked in source.

A package file can consist of any items in the package's [contents][package-contents],
which include:

- Files inside `meta/`
- Files in the base package

Note: For components, it's recommended to track the output component manifest (that is,
`meta/<component_name>.cm`) at the `expected_files_exact` level for an API surface. This
ensures that anything that changes the package's functionality (which in turn changes its
component manifest content hash) will be surfaced.

## Explore package contents {:#explore-package-contents .numbered}

Provide a list of the items that make up a package in the package's manifest file
(commonly named `package_manifest.json`)

See an example manifest file below:

```json {:.devsite-disable-click-to-copy}
{
  "version": "1",
  "repository": "fuchsia.com",
  "package": {
    "name": "example_package",
    "version": "0"
  },
  "blobs": [
    {
      "source_path": "path/to/meta.far",
      "path": "meta/",
      "merkle": "CONTENT_HASH",
      "size": 0
    },
    {
      "source_path": "path/to/bin/example",
      "path": "bin/example",
      "merkle": "CONTENT_HASH",
      "size": 0
    },
    {
      "source_path": "path/to/shared/bar.so",
      "path": "lib/bar.so",
      "merkle": "CONTENT_HASH",
      "size": 0
    }
  ]
}

```

In the example above, possible API surface items (besides the `meta.far`) are in the
`blobs/path` entities, which include:

- `bin/example`
- `lib/far.so`

The `ffx package far list` command allows you to explore the contents of the `meta.far`,
for example:

```none {:.devsite-disable-click-to-copy}
$ ffx package far list path/to/meta.far
+-------------------------------+
| path                          |
+===============================+
| meta/contents                 |
+-------------------------------+
| meta/fuchsia.abi/abi-revision |
+-------------------------------+
| meta/component_name.cm        |
+-------------------------------+
```

In total, the possible API surface items (including `meta.far`) from this example now include:

- `bin/example`
- `lib/far.so`
- `meta/contents`
- `meta/fuchsia.abi/abi-revision`
- `meta/component_name.cm`

## Contribute to an API in the SDK {:#contribute-to-an-api-in-the-sdk .numbered}

Tip: See this [example Gerrit change][example-gerrit-change-01] for the steps described in
this section.

To contribute a package to the Fuchsia API surface, follow the guidelines below:


1. Prepare to [request a code review][request-a-code-review] from an API council member.

   This member may ask for adjustments to your API surface (see this
   [example code review][example-code-review] of an SDK package).

1. Create a `sdk_fuchsia_package` target for your SDK package
   ([example][example-build-gn-01]).

   This will likely live next to the `fuchsia_package` target that is being
   exported.

1. Pay attention to the declared API surface, represented by the
   `expected_files_exact`and `expected_files_present` parameters of the
   `sdk_fuchsia_package` target.

   (see [Create a package API surface](#create-a-package-api-surface).)

1. Add the `sdk_fuchsia_package` target to the dependencies of the
   `sdk_molecule` ("core_packages") target in the `//sdk/BUILD.gn` file
   ([example][example-build-gn-02]).

   To build the IDK target (which is expected to fail its golden file check),
   run the following command:

   ```posix-terminal
   fx build final_fuchsia_idk
   ```

1. Follow the printed instructions for copying the generated `content_checklist.json`
   file to its golden file location.

1. Confirm that the generated `content_checklist.json` file covers the desired
   API surface ([example][example-content-checklist]).

   If not, adjust the `expected_files_exact` and `expected_files_present` parameters
   and repeat the previous build step.

1. Build the IDK target after fixing any other SDK manifest-related changes (including updating
   the `//sdk/manifests/fuchsia_idk.manifest` files).

   ```posix-terminal
   fx build final_fuchsia_idk
   ```

1. **(Optional)** Enter the SDK build directory and preview your SDK package manifest.

   From your build directory (for example, `out/default`), do the following:

   1. Change the directory to `sdk/archive`, for example:

      ```posix-terminal
      cd sdk/archive
      ```

   1. Run the following command:

      ```posix-terminal
      mkdir output && tar zxvf fuchsia_idk.tar.gz -C output && cd output
      ```

      The package manifest of your package targeting x64 will be found at
      `packages/{PACKAGE_DISTRIBUTION_NAME}/x64/release/package_manifest.json`

<!-- Reference links -->

[packages]: /docs/concepts/packages/package.md
[api-surface]: /docs/glossary/README.md#fuchsia-api-surface
[idk]:/docs/development/idk/README.md
[promoting-api]: /docs/contribute/sdk/README.md#promoting_an_api_to_the_partner_category
[sdk-fuchsia-package-gni]: /build/packages/sdk_fuchsia_package.gni
[package-contents]: /docs/concepts/packages/package.md#structure-of-a-package
[example-gerrit-change-01]: http://fxrev.dev/c/fuchsia/+/895023
[example-build-gn-01]: http://fxrev.dev/c/fuchsia/+/895023/39/src/lib/fuchsia-component-test/realm_builder_server/BUILD.gn
[example-build-gn-02]: https://cs.opensource.google/fuchsia/fuchsia/+/9796254028425f4d948ca5976976dabba5c40f58:src/lib/fuchsia-component-test/realm_builder_server/BUILD.gn;l=89-94?q=src%2Flib%2Ffuchsia-component-test%2Frealm_builder_server%2FBUILD.gn&ss=fuchsia
[example-content-checklist]: https://cs.opensource.google/fuchsia/fuchsia/+/9796254028425f4d948ca5976976dabba5c40f58:sdk/packages/realm_builder_server/release/content_checklist.api
[request-a-code-review]: /docs/development/source_code/contribute_changes.md#request-a-code-review
[example-code-review]: http://fxrev.dev/c/fuchsia/+/895023
