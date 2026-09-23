# Third-party source code management

Third-party code is part of the Fuchsia checkout but is neither copyrighted by
the Fuchsia authors nor subject to Fuchsia's [license]. In other words, any code
that is not 100% owned by the Fuchsia authors is managed as third-party code.

The Fuchsia project maintains copies of third-party code dependencies under the
`//third_party/` directory in the checkout. This is also known as vendoring.
Vendoring ensures that third-party code is served from Fuchsia-owned source
repositories and is served at revisions that are known to work with other code
in the Fuchsia checkout.

When adding third-party code, follow the steps below to ensure the code complies
with the Fuchsia project policies.

## Before you start

All external code must go through the
[Open Source Review Board (OSRB) process][osrb-process] to be added to the
Fuchsia Platform Source Tree. Once the OSRB request is approved, continue with
the steps below.

If the dependency or sub-dependency will not be used in the Fuchsia checkout,
specify that in your OSRB request and state that it will be blocklisted from the
Fuchsia checkout via [adding to this
blocklist.](https://fuchsia.googlesource.com/fuchsia/+/main/scripts/shac/mirror_blocklists.star)
Most licensing issues arise if it is to be used in the Fuchsia checkout.

### Language-specific guides

If you are adding Rust, Go or Python dependencies, follow the guides below:

-   **Rust**: Follow the [external Rust crates][rust-third-party] guide.

-   **Go**: See [`//third_party/golibs/`][golibs].

-   **Python**: Follow the [external Python packages][pylibs] guide.

For all other languages, continue with the steps below.

## Get the code

All external code must follow the third_party source layout below (using
`googletest` as example):

```none {:.devsite-disable-click-to-copy}
root [fuchsia.googlesource.com/fuchsia]
  third_party/
    googletest/
      src/ [fuchsia.googlesource.com/third_party/github.com/google/googletest]
      BUILD.gn
      OWNERS
      README.fuchsia
```

`//third_party/googletest/src/` is the root of the
[Fuchsia-owned mirror repository][third-party-googletest], that contains a copy
of the [upstream repository][googletest] for `googletest`. (*Note:* For Python
repositories, replace `/src` with `/<module_name>` to follow Python's
convention. This convention is expected by common Python tools like
[pyright][pyrightconfig].)

The `//third_party/googletest/` directory is part of the
[`fuchsia.git`][fuchsia-git] repository.

`//third_party/googletest/BUILD.gn` defines build targets for the `googletest`
library. Since this file belongs to [`fuchsia.git`][fuchsia-git] (not the
[`googletest` repository][third-party-googletest]), it can be updated in
lockstep with other Fuchsia `BUILD.gn` files that depend on `googletest`. This
makes build refactors and other large-scale changes easier.

Additional files that are required to adapt the third-party code to the Fuchsia
project may be present under (in this case) `//third_party/googletest`.

### Add OWNERS

Each dependency must have an associated [`OWNERS`][owners] file. Because it's
defined in `fuchsia.git`, it is possible to include owners from other files
elsewhere in the Fuchsia project.

The OWNERS file must either list two Fuchsia developer accounts as the first two
lines or include a `file:` directive to another OWNERS file. This will ensure
accountability for maintenance of the code over time.

The OWNERS are typically the owners of the code that use the dependency, unless
specified otherwise.

The dependency's OWNERS help keep Fuchsia and its users safe by: * Removing the
dependency when/if it is no longer needed * Updating the dependency when a
security or stability bug is fixed upstream * Helping ensure the Fuchsia feature
that uses the dependency continues to use the dependency in the best way, as the
feature and the dependency change over time.

### Add README.fuchsia

You need a README.fuchsia file with information about the project from which
you're reusing code. Check out [`README.fuchsia`][readme-fuchsia] for the list
of required fields to include.

### Get a review

All third-party additions and substantive changes like re-licensing need the
following sign-offs:

*   Get the code reviewed as instructed in the [OSRB approval][osrb-process].
   * If the dependency or sub-dependency will not be used in the Fuchsia checkout,
   specify that in your OSRB request and state that it will be blocklisted from the
   Fuchsia checkout via [adding to this
   blocklist.](https://fuchsia.googlesource.com/fuchsia/+/main/scripts/shac/mirror_blocklists.star)
   Most licensing issues arise if it is to be used in the Fuchsia checkout.
*   If the third-party project is security-critical (as defined in
    [`README.fuchsia`][readme-fuchsia]), include someone in
    `security-dev@fuchsia.dev` to review the change.

### Exceptional cases

Most third-party dependencies can follow the layout described above. However, a
small fraction of dependencies that are subject to uncommon circumstances are
managed differently.

Having exotic dependencies can increase complexity and maintenance costs, which
are incurred by direct dependencies of the third-party code. Additionally, they
add complexity to common global maintenance tasks such as:

-   Performing git administration tasks.
-   Updating and maintaining toolchains.
-   Responding to disclosed security vulnerabilities by updating vulnerable
    third-party code from upstream sources.
-   Refactoring build rules, such as to enforce new compile-time checks.

Please exercise careful deliberation when stepping off the beaten path.

## Migrating legacy third-party code to current layout

Bringing all the existing //third_party code to the layout documented above is
WIP, and contributions are welcome.

To migrate legacy third-party repositories to this layout, follow these steps:

1.  Update the manifest.

    1.  Replace `path` (not `name`) of the existing third-party project at
        `//third_party/<name>` with `//third_party/<name>/src`, while keeping
        the revision unchanged.

    1.  Update [`//.gitignore`][gitignore] so that `//third_party/<name>` is
        tracked but `//third_party/<name>/src` is not tracked.

    Then run `jiri update -local-manifest-project=fuchsia` which will move the
    project to its new location in your local checkout.

1.  Move Fuchsia-specific `BUILD.gn` files into fuchsia.git.

    1.  Copy `BUILD.gn` files from `//third_party/<name>/src` into
        `//third_party/<name>` (now part of fuchsia.git).
    1.  In the copied `BUILD.gn` files, update references to paths to
        third-party files in the form of `//third_party/<name>/` to the form of
        `//third_party/<name>/src/`.
    1.  Copy `OWNERS` from `//third_party/<name>/src` to `//third_party/<name>`,
        or create it if it does not exist. Review the `OWNERS` file to ensure
        that it follows the [best practices][owners-best-practices].
    1.  Copy `README.fuchsia` from `//third_party/<name>/src` to
        `//third_party/<name>`. Review the contents of this file and ensure that
        the metadata is correct. In uncommon cases there are modifications made
        to third-party code in third-party repositories, and such changes are
        listed in `README.fuchsia`. Local modifications will often require you
        to make special accommodations that are not covered in this guide.
    1.  Review `//third_party/<name>/src` for any other first party `.gni` files
        and move those to `//third_party/<name>` as well.
    1.  Update `//third_party/<name>/BUILD.gn` (and other files containing
        source paths such as `.gni` files) to use the new source location
        `//third_party/<name>/src`. This requires updating all sources,
        including directory paths and more.

1.  Turn `//third_party/<name>/src` into a mirror.

    Change `//third_party/<name>/src` to track upstream such that it only has
    upstream changes in its `git log`. You can do this by updating the manifest
    to reference an upstream commit hash.

    Example: [http://tqr/427570](http://tqr/427570)

1.  Commit and push your changes. All of these changes can be done in a single
    CL.

You can validate the changes locally by running `jiri update
-local-manifest-project=fuchsia`, then building (such as with `fx build`).

## Additional reading

-   [Fuchsia open source licensing policies][oss-licensing]
-   [Source code layout][source-layout]

[fuchsia-git]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main
[gitignore]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/.gitignore
[golibs]: /third_party/golibs/
[googletest]: https://github.com/google/googletest
[license]: /LICENSE
[osrb-process]: /docs/contribute/governance/policy/osrb-process.md
[oss-licensing]: /docs/contribute/governance/policy/open-source-licensing-policies.md
[owners]: /docs/development/source_code/owners.md
[owners-best-practices]: /docs/development/source_code/owners.md#best_practices
[pylibs]: /third_party/pylibs/README.md
[readme-fuchsia]: /docs/development/source_code/third-party-metadata.md
[rust-third-party]: /docs/development/languages/rust/external_crates.md
[source-layout]: /docs/development/source_code/layout.md
[third-party-googletest]: https://fuchsia.googlesource.com/third_party/github.com/google/googletest/
[pyrightconfig]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/pyrightconfig.json
