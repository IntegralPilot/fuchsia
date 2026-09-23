# Overview

`bazel2gn` is a tool for syncing build targets between BUILD.bazel and BUILD.gn
files in fuchsia.git. It works by converting build targets from BUILD.bazel
files and replacing the corresponding targets in BUILD.gn.

## GN to Bazel migration instructions

### To migrate new targets from foo/bar/BUILD.gn to Bazel

NOTE: Don't forget step 5 in the process, to make sure your targets are
continuously validated in CQ.

1. Create foo/bar/BUILD.bazel if it doesn't exist already.
2. Implement the target you want to migrate in foo/bar/BUILD.bazel.
3. Run `fx bazel2gn -d foo/bar` to sync the targets defined in
   foo/bar/BUILD.bazel to foo/bar/BUILD.gn.
4. Remove old GN targets you've migrated from foo/bar/BUILD.gn.
5. Add `//foo/bar:verify_bazel2gn` to the `deps` of
   `//build:bazel2gn_verifications` in `//build/BUILD.gn`.

### To re-sync targets managed by `bazel2gn`

* Run `fx bazel2gn`, which will sync your target if it's a dep in
  `//build:bazel2gn_verifications`, plus all other deps of that group.
* Or, run `fx bazel2gn -d path/to/dir`, if you only want to sync targets in a
  specific directory.

## Motivation

This tool is created to facilitate the GN to Bazel migration in fuchsia.git.
During the migration, it is unavoidable to have targets that need to be
buildable in both GN and Bazel, and it is costly to manually sync those targets.
This tool allows us to maintain the source of truth in Bazel, and automatically
sync those targets to GN.

## Design ideas

There are two main sources of differences between BUILD.gn and BUILD.bazel:

* Differences between GN templates and Bazel rules for the same target;
* Differences between Starlark and GN languages.

Examples of these differences and ideas for tackling them during conversion are
discussed in the sections below. For implementation details, please refer to the
Go sources in this directory.

### GN templates vs Bazel rules

It's very common for GN templates and Bazel rules to have different attributes
for the same target type. Some of them are simple naming differences,
for example `srcs` in Bazel rules are usually spelled `sources` in GN templates.
This can be converted with hardcoded mappings. Others are more complicated,
for example, `go_library` from `rules_go` in Bazel requires sources for
[embed][goembed] to be specified in [`embedsrcs`][embedsrcs], and our
`go_library` in GN simply mixes embed sources with other sources in `sources`.
To bridge this gap, it is preferred to modify template implementation in GN
while maintaining backward compatibility, because:

* The build team owns most of the GN templates, making them easier to change;
* We'll eventually remove the GN targets, so it's better to keep Bazel targets
    as idiomatic as possible.

### Starlark vs GN

[Starlark][starlark] is the chosen language of Bazel for defining build targets,
while GN has its [own language][gnreference] for the same purpose. Starlark is
more declarative than GN. For example, to conditionally build a list in GN, you
can use if statements to imperatively add elements to the list, while in Bazel
this is done by concatenating results returned by the `select` function.

#### Conditional targets

It's common to have targets in the build graph that are conditionally defined.
For example, host tools should only be built for the host. In GN, this is
expressed with if clauses. In Bazel, because if clauses are not supported in
BUILD.bazel files, this is done by setting the `target_compatible_with`
attribute when defining targets. So to do the conversion from Bazel to GN,
`bazel2gn` needs recognize `target_compatible_with` attributes in Bazel, and
know how to convert its value to GN if conditions. For example,
`target_compatible_with = HOST_CONSTRAINTS` in Bazel maps to
`if (is_host) { ... }` in GN. Note `target_compatible_with` is a list, so it's
possible for it to be converted to several GN conditions ored together.

#### Conditional values

It's common in Bazel to conditionally assign values to attributes. For example,
the `srcs` attribute of a target can be conditionally assigned on different
platforms:

NOTE: Not all attributes support `select()` values. Consult the rule's
documentation for details.

```bzl
rust_library(
	name = "list_concatenation",
	srcs = [
		"foo.rs",
	] + select({
		"@platforms//os:fuchsia": [ "fuchsia.rs" ],
		"@platforms//os:linux": [ "linux.rs" ],
	}),
)
```

In GN, this is synced back to if-else-if blocks:

```gn
rustc_library("list_concatenation") {
	sources = []
	sources += [
		"foo.rs",
	]
	if (is_fuchsia) {
		sources += [
			"fuchsia.rs",
		]
	} else if (is_linux) {
		sources += [
			"linux.rs",
		]
	}
}
```

### Comment annotations

`bazel2gn` supports several comment annotations in `BUILD.bazel` files to
control the conversion process. These annotations can be used to alter the
behavior of `bazel2gn` when converting parts of the `BUILD.bazel` where the
annotations are placed.

#### `@bazel2gn:skip`

This annotation tells `bazel2gn` to skip the annotated statement. It is useful
for ignoring targets that are no longer useful in GN. This annotation must be
placed right above the statement it is annotating to take effect.

**Example:**

```bzl
rustc_library(
    name = "foo",
    srcs = ["foo.rs"],
)

# @bazel2gn:skip
rustc_library(
    name = "bar",
    srcs = ["bar.rs"],
)
```

**Converted GN:**

```gn
rustc_library("foo") {
  sources = [ "foo.rs" ]
}
```

#### `@bazel2gn:clear`

This annotation tells `bazel2gn` an assignment is an explicit clear.
It is useful when you want to explicitly set an attribute to empty in GN,
which would otherwise result in list concatenation in GN. This annotation must
be placed on the same line as the assignment it is annotating to take effect.

NOTE: This currently only applies to the `configs` attribute in GN.

**Example:**

```bzl
cc_library(
    name = "foo",
    srcs = ["foo.cc"],
    copts = [],  # @bazel2gn:clear
)
```

**Converted GN:**

```gn
source_set("foo") {
  sources = [ "foo.cc" ]
  configs = []
}
```

#### `@bazel2gn:path_overwrite:<path>`

This annotation allows you to overwrite a specific path in a list. It is useful
when a file path in Bazel is different from its path in GN. This annotation must
be placed on the same line as the path it is overwriting to take effect.
The specified value will be wrapped in double quotes.

**Example:**

```bzl
cc_library(
    name = "foo",
    srcs = [
        "foo.cc",
        "bar.cc",  # @bazel2gn:path_overwrite:bar_replaced.cc
    ],
)
```

**Converted GN:**

```gn
source_set("foo") {
  sources = [
    "foo.cc",
    "bar_replaced.cc",
  ]
}
```

#### `@bazel2gn:raw_overwrite:<value>`

<!-- TODO(https://fxbug.dev/543568916) Specify where this and other annotations
     are supported once a decision is made about which scenarios to support
     and that is reflected in the implementation. -->
Where supported, this annotation allows you to overwrite a value with the exact
text that follows the annotation. It is useful when a non-path value is
different in GN, including cases where GN features like `rebase_path()` must be
used. This annotation must be placed on the same line as the value it is
overwriting to take effect.

**Example:**

```bzl
cc_library(
    name = "foo",
    ...
    ldflags = [
        "-Wl,--version-script=sdk/lib/fdio/fdio.ld",  # @bazel2gn:raw_overwrite:"-Wl,--version-script=" + rebase_path("fdio.ld", root_build_dir)
    ],
)
```

**Converted GN:**

```gn
source_set("foo") {
    ...
  ldflags =
      [ "-Wl,--version-script=" + rebase_path("fdio.ld", root_build_dir) ]
}
```

#### `@bazel2gn:transformer=<transformer>`

This annotation tells `bazel2gn` to apply a transformer to the annotated
statement. It is useful for controlling the conversion process.

**Example:**

```bzl
COMMON_DEPS = ["//third_party/rust_crates/vendor:lock_api"]
```

**Converted GN:**

```gn
COMMON_DEPS = [ "//third_party/rust_crates:lock_api" ]
```

The currently supported transformers are:

* `visibility`: converts Bazel visibility to GN visibility.
* `deps`: converts Bazel dependencies to GN dependencies.
* `configs`: converts Bazel copt to GN configs.
* `file_paths`: converts Bazel file paths to GN file paths.

### Third-party dependency translation

`bazel2gn` automatically translates Bazel external repository target labels
(e.g., `@com_google_googletest//:gtest`, `@com_googletest_gtest//:gtest`,
`@re2//:re2`, `@boringssl//:crypto`) to their equivalent GN labels
(e.g., `//third_party/googletest:gtest`, `//third_party/re2`,
`//third_party/boringssl:crypto`).

The mapping between Bazel target labels and GN labels is defined in
[`third_party_target_map.json`](third_party_target_map.json).

If `bazel2gn` encounters an external repository dependency (starting with `@`)
that is not defined in `third_party_target_map.json` and has no manual overwrite
directive:
1. It prints a warning to `stderr`:
   `WARNING: unable to translate Bazel label <label> to GN`
2. It appends an inline comment in the generated GN file:
   `# BAZEL2GN_WARNING: Unknown Bazel repository name`

[embedsrcs]: https://github.com/bazel-contrib/rules_go/blob/master/docs/go/core/rules.md#go_library-embedsrcs
[goembed]: https://pkg.go.dev/embed
[starlark]: https://bazel.build/rules/language
[gnreference]: https://gn.googlesource.com/gn/+/main/docs/reference.md
