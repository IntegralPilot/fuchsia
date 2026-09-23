# GN root targets

The Fuchsia build uses GN's new `root_patterns` feature to considerably
reduce the size of the GN and Ninja build graphs. This results in significantly
faster `gn gen` time, and Ninja startup time.

However, this feature changes GN's default behavior when parsing `BUILD.gn` files
in ways that can be surprising. This document explains how.

## GN default behavior

For historical reasons, GN will instantiate every target defined in a `BUILD.gn` file
when the latter is evaluated in the context of the _default toolchain_, _even if nothing
really depends on them_.

As an example, consider the following three `BUILD.gn` files that each define two targets,
with a few dependencies between them:

```none {:.devsite-disable-click-to-copy}
 _//BUILD.gn____    _//foo/BUILD.gn__    _//bar/BUILD.gn_
|               |  |                 |  |                |
|   A           |  |         D -------------> E          |
|               |  |                 |  |                |
|      B --------------> C           |  |         F      |
|_______________|  |_________________|  |________________|
```

When GN parses this build plan the following happens:

- GN starts by loading `//BUILD.gn`, and evaluating it. Because it
  uses the default toolchain, it creates targets for all entries in
  that file, i.e. `//:A` and `//:B`.

- GN follows dependencies of the targets it just created, since
  `//:B` depends on `//foo:C`, it first loads `//foo/BUILD.gn` and
  evaluates it.

- Because it is still in the default toolchain, it instantiates all
  targets in that file too, and thus creates `//foo:C` and `//foo:D`,
  even though the latter is not a dependency of `//:A` or `//:B`

- It follows dependencies again, and will load `//bar/BUILD.gn`,
  evaluate it, and create all targets defined here, hence `//bar:E`
  and `//bar:F`

This leads to the final build graph containing many more targets than
needed, if one assumes that `//BUILD.gn` represents the root of the
graph.

## GN `root_patterns`

It is possible to change this default behavior by setting
[`root_patterns`][gn-root-patterns]{:.external} in the `.gn` file,
or using the `--root-pattern=<pattern>` command-line option (once or
more).

These define a list of target label patterns, used to filter which
of the non-dependency targets, present in `BUILD.gn` files evaluated
in the default toolchain, to create.

For example, using `gn gen --root-pattern=//:*` with the same build
plan will change GN's behavior in the following way:

- GN starts by loading `//BUILD.gn` and evaluates it. Because it is
  in the default toolchain, any target in the file that is created,
  because they match the pattern (`//:*` really means "any target
  in `//BUILD.gn`"). It thus creates `//:A` and `//:B` as before.

- GN follows dependencies, then loads `//foo/BUILD.gn` and evaluates
  it. It creates `//foo:C` because this is a direct dependency of
  one of the previous targets it created. However, it does _not_
  create `//foo:D`, as its label does not match the pattern `//:*`.

- GN stops here, since it didn't create `//foo:D`, it has not reason
  to load `//bar:BUILD.gn`

Thus GN creates 3 targets, instead of 6 in the final build graph.

## Practical results

In practice, using this feature reduces the size of our GN build
graph considerably, speeding up the `gn gen` time as well. For
example, using an `fx set minimal.x64` configuration:

```none {:.devsite-disable-click-to-copy}
                        Default     --root-pattern=//:*    Reduction

Target count             183761              48375          -73%
Ninja files size (MiB)    571.7              180.2          -68%
`fx set` peak RAM (GiB)    5.02               2.89          -42%
`gn gen` time (s)          14.9               6.15          -58%
`fx set` time (s)          16.0               6.77          -57%
```

## The `//:root_targets` target.

The `//BUILD.gn` file now defines a top-level target named `root_targets`
that can be used to add dependencies to targets that absolutely must
be in the build graph even though nothing else depends on them.

This is critical for a few cases:

- Some special `generated_file()` targets whose output is used
  as an implicit input by other targets, but which cannot be depended
  on directly.

- A few targets that hard-coded infra tools expect to always be built
  in the tree.

- Some targets that are expected by some builder configurations that
  incorrectly didn't list them in their `universe_package_labels`,
  and just assumed their existence due to other required targets
  defined in the same `BUILD.gn` file.

Adding to this list should be minimized. It is always better to find
a real transitive dependency from any of the other top-level
`//BUILD.gn` targets if you really need something to always be
defined in the final Ninja build manifest.

[gn-root-patterns]: https://gn.googlesource.com/gn/+/main/docs/reference.md#other-help-topics-gn-file-variables
