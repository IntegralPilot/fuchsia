# Publish a CIPD symbols package for ELF binaries {#publish-a-symbols-cipd-package}

A symbols package is used for symbolization in logs and
debugging crashes. A CIPD prebuilt package that contains Fuchsia ELF binaries
needs to have a companion symbols package that contains debug information for
the binaries.

## Requirements for a symbols package

*   Each ELF binary in the original CIPD package must include
    an ELF `NT_GNU_BUILD_ID` note section, which includes a unique `build-id`
    hash value that uniquely identifies its non-debug-related content.

    This note is created at link time when producing the ELF binary. Recent
    versions of GCC, or the prebuilt Fuchsia Clang toolchain, produce it
    automatically. However, regular Clang requires passing a special linker
    flag (that is, `-Wl,--build-id`).

    Note: To print the `build-id` of an existing library, use either one of
    `file <LIBRARY>` or
    `readelf -n <LIBRARY> | grep "Build ID"`

*   The symbols package must use the directory layout typical of `.build-id`
    directories used to store
    [debug information in separate files](https://sourceware.org/gdb/current/onlinedocs/gdb/Separate-Debug-Files.html){: .external}.

    This means that each file must be stored as `<xx>/<xxxxxxxxxx>.debug`,
    where `<xx>` and `<xxxxxxxxx>` are hex strings derived from the `build-id`
    hash value, of the unstripped binary. Each of the `.debug` files should be
    a copy of the unstripped ELF binary itself, not just the extracted debug
    information. These files should map to the stripped ELF binary with
    the same `build-id` from the original package.

    Note: The symbols package needs to not include the top-level `.build-id`
    directory.

    This example shows the directory structure of a CIPD symbols package with
    unstripped ELF binaries:

    ```none
    1d/
      bca0bd1be33e19.debug
    1f/
      512abdcbe453ee.debug
      90dd45623deab1.debug
    2b/
      0e519bcf3942dd.debug
    3d/
      aca0b11beff127.debug
    5b/
      66bc85af2da641697328996cbc04d62b84fc58.debug
    ```

*   The symbols package must use the same
    [version identifiers](/docs/development/prebuilt_packages/publish_prebuilt_packages_to_cipd.md#set-cipd-package-versioning)
    (tags) as the original CIPD package they refer to. This allows them to
    be rolled together.

*   If several CIPD packages that contain stripped ELF binaries are rolled
    together (using the same version identifiers), then grouping the debug
    symbols for all of them in a single CIPD symbols package is acceptable,
    but not required.

*   The CIPD path for the symbols package needs to use the
    the following suffix:

    ```none
    -debug-symbols-<ARCH>
    ```

    For example,
    `myproject/fuchsia/mypackage-debug-symbols-amd64` contains the symbols
    for the `myproject/fuchsia/mypackage-amd64` prebuilt package.

*   The Jiri checkout path for all symbols package must be
    `${FUCHSIA_DIR}/prebuilt/.build-id`.

## Generate a symbols package {#generate-symbols-package}

To generate a symbols package, you need to:

*   Compile all your ELF binaries with DWARF debug information (for example,
    passing the `-g` flag to the compiler, even in release mode).

*   Produce an `NT_GNU_BUILD_ID` note for the ELF binaries.

    This is the default on recent GCC versions and the Fuchsia prebuilt
    Clang toolchain, but regular Clang requires passing a special flag
    (`-Wl,--build-id`) to the linker.

The recommended way to generate a symbols package is to use the `buildidtool`
prebuilt tool:

* For out-of-tree builds, you can get this tool from
  [CIPD](https://chrome-infra-packages.appspot.com/p/fuchsia/tools/buildidtool){:# external}.

Use the `buildidtool` to generate the symbol package directory structure:

```bash
# Example assuming your unstripped binary is at out/libfoo.so and you want to
# create the symbols directory at ./symbols

UNSTRIPPED_BINARY=out/libfoo.so
SYMBOLS_DIR=./symbols
BUILDIDTOOL="${FUCHSIA_DIR}/prebuilt/tools/buildidtool/linux-x64/buildidtool" # Adjust path as needed

mkdir -p "${SYMBOLS_DIR}" # Ensure symbols directory exists

"${BUILDIDTOOL}" --build-id-dir="${SYMBOLS_DIR}" -entry .debug="${UNSTRIPPED_BINARY}" -stamp "$(mktemp)"
```

Repeat this `buildidtool` command for each unstripped ELF binary in your
prebuilt package to populate the `symbols/` directory.

Note: Don't forget to strip your ELF binaries for your main prebuilt CIPD
package. You can use `llvm-objcopy --strip-all` or `llvm-strip`.

Then, upload the content of the `symbols/` directory (not the directory itself)
as your symbols package to CIPD.

## Test symbols packages locally

Before testing symbols packages locally, you should understand what a `jiri`
entry for a symbols package looks like:

```xml
<package name=$CIPD_PATH-debug-symbols-$cpu # Example: ~/fuchsia/mypackage-debug-symbols-amd64
         path="prebuilt/.build-id"
         version=$CIPD_VERSION
         attributes="debug-symbols,debug-symbols-$cpu,debug-symbols-$project"/>
```

**Explanation of attributes:**

*   `path="prebuilt/.build-id"`: All symbol packages must be downloaded to the
    `${FUCHSIA_DIR}/prebuilt/.build-id` directory in your project checkout.
*   `attributes="debug-symbols,debug-symbols-$cpu, debug-symbols-$project"`:
    This controls when the symbols package is downloaded.
    *   `debug-symbols-$cpu` (e.g., `debug-symbols-arm64`, `debug-symbols-amd64`).
    *   `debug-symbols-$project`: This attribute is used to download symbols
        packages relevant to your specific project. For local testing, see the
        instructions below.
    *   `debug-symbols`: General convention.

To be able to use `jiri` to fetch a symbols package for a local test, you need
to configure `jiri`. For example, to configure jiri to fetch
`debug-symbols-$project`:

```posix-terminal
jiri init -fetch-optional=debug-symbols-$project
```

Then, retrieve the packages:

```posix-terminal
jiri fetch-packages --local-manifest
```

Keep in mind that opting into symbol package download through Jiri attributes
can significantly increase the disk space used by your project checkout. You can
remove opted-in attributes by manually editing the `.jiri_root/attributes.json`
file in your checkout's top-level directory.
