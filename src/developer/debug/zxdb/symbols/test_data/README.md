# Test data for zxdb

This is used for the symbol parsing tests.

There are two flavors of tests. Some build the symbol test and run on that, expecting to find
certain symbol patterns.

Tests that rely heaving on the binary layout use the checked-in version to avoid compiler and
platform variations.

## To generate the binaries

  * On x64 copy the generated `libzxdb_symbol_test.so` that includes symbols to
    `libsymbol_test_so.targetso`. As of this writing, the compiled file will be something
    like: `$FUCHSIA_OUT_DIR/arm64-shared/lib.unstripped/libzxdb_symbol_test.so`

  * Copy the stripped version from `$FUCHSIA_OUT_DIR/arm64-shared/libzxdb_symbol_test.so` to
    `libsymbol_test_so_stripped.targetso`

# Rust symbol data

Rust specific symbols are too large to check into the git tree, so they are uploaded to cipd. These
will be downloaded with the default Fuchsia jiri configuration, unlike the large tests below.

# Large tests

There are some additional binaries that are too large to include with every checkout. These are
used by the `zxdb_large_tests` target and appear in `//prebuilt/test_data/debug/large_test_data`.

To make these work you need to download them and enable the target that requires them.

### Downloading the large test data

This package is Google-internal-only so we have the option to add different binaries regardless of
licences or whether they're unreleased. To opt-in to downloading these binaries:

```
jiri init -fetch-optional=zxdb-large-test-data
jiri update
```

If that doesn't work, it's possible you need the internal version of the integration repository
checked out. See the Google-internal checkout instructions for how to do this.

### Enabling the zxdb large tests

There is a GN build flag `include_zxdb_large_tests` which defaults to `false`. This controls whether
the target `out/<arch>/host_x64/zxdb_large_tests` are compiled. To set this to true, run this,
substituting `<arch>` with your build directory:

```
fx gn args out/<arch>
```

and add a line to the bottom of the file:

```
include_zxdb_large_tests = true
```

## Adding to the large test data

The file `large_test_data.yaml` in this directory is the CIPD specification for the package. To add
files, add them to `//prebuilt/test_data/debug/large_test_data` alongside the existing ones. Add
the file names to the .yaml file. _Be sure to add licenses and READMEs for each file added,
including whether it needs to be internal-only or not._

Each package is identified by a version tag. This tag will be used later to specify which package
to fetch. We use the tag name `git_revision:...` where "..." is the git hash of the current
tip-of-tree. This way it's always unique (otherwise the tags are basically free-form).

Copy `large_test_data.yaml` from this directory to `//prebuilt/test_data/debug/large_test_data`
since CIPD wants everything to be in the same directory while it's working. Be sure to check in the
final version to this directory so it's always current.

The atomic way to make a package and upload with a version tag is (don't forget to replace the git
hash!)

```
cipd pkg-build -pkg-def large_test_data.yaml -out large_test_data.cipd
cipd pkg-register -tag git_revision:GIT_HASH_GOES_HERE large_test_data.cipd
```

The other way to do this is make and upload a package and set the version tag later. To build and
upload:

```
cipd create -pkg-def large_test_data.yaml
```

To list the instances of this package once uploaded:

```
cipd instances fuchsia_internal/test_data/debug/large_test_data
```

To see the version tag, specify the "instance ID" from the above list:

```
cipd describe fuchsia_internal/test_data/debug/large_test_data -version FZIh6hPkZOZa7vbUXjLjUnfzu3tYxnZ-NRHLyf2HnrMC
```

To explicitly set a version tag for an instance:

```
cipd set-tag fuchsia_internal/test_data/debug/large_test_data -version FZIh6hPkZOZa7vbUXjLjUnfzu3tYxnZ-NRHLyf2HnrMC -tag git_revision:fa316074ae0f53a2562c76cb4637b6f2892b02f7
```

## Adding to or updating rust test data

Adding files or updating the binaries contained in the rust directory is exactly the same as for the
large test data described above, except use the `rust_test_data.yaml` file instead. New executables
can be added to the CIPD package with additional rust targets in the rust directory, make sure to
update the `dep` in the `rust_symbols` target.

If a binary in this package needs to be updated, you'll need to uncomment the dep in the
`rust_symbols` target in `//src/developer/debug/zxdb/symbols/BUILD.gn`.

## Fetch configuration

To cause the package to be downloaded, there is an entry in
`//manifests/prebuilts`. Substitute the tag you specified for the CIPD package
in the "version" field in that file.

Don't forget to run `//manifests/update-lockfiles.sh` after making changes to `prebuilts`!

Note that the "attributes" attribute specified that this package won't be downloaded unless the user
opts-in as described earlier.

To test that the updated manifest works:

```
jiri fetch-packages -local-manifest-project=fuchsia
```
