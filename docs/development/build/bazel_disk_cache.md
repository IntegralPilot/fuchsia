# Bazel disk cache

This document describes how to configure and use a local disk cache for Bazel
artifacts in your Fuchsia development environment.

## Overview {:#overview}

The Fuchsia build system uses Bazel for building certain parts of the system.
By default, Bazel stores its cache inside the build directory (e.g.,
`out/default/bazel-*`). If you clean your build directory (for example, by
running `fx clean-build` or `rm -rf out`), this cache is lost, and Bazel
must rebuild everything from scratch.

You can configure a persistent, shared disk cache that survives build
directory cleans and can be shared across multiple Fuchsia checkouts and/or
build directories on the same machine.

## Benefits of a shared disk cache {:#benefits-of-a-shared-disk-cache }

* **Faster Clean Builds:** When you run a clean build, Bazel can retrieve
  previously built actions and artifacts from the disk cache instead of
  rebuilding them.

* **Shared Across Checkouts:** If you work with multiple Fuchsia checkouts
  on the same machine, they can all share the same disk cache. Artifacts
  built in one checkout can be reused in another, saving time and disk
  space.

* **Shared Across Build Directories:** If you switch between different
  build directories in the same checkout, actions that are identical between
  them (including input contents) will automatically benefit from the
  cache too.

* **Shared Across Branches:** If you frequently switch between branches,
  while using the same build directory, incremental builds will also
  benefit from the disk cache.

* **Complements Remote Caching (RBE):** Bazel's disk cache is compatible with
  remote caching. Even when Remote Build Execution (RBE) is enabled, a local
  disk cache is often faster because it avoids network latency when retrieving
  cached artifacts. It also allows you to work offline for targets you have
  already built.

## Enabling the Bazel disk cache {:#enabling-the-bazel-disk-cache}

To enable the disk cache, set the `FUCHSIA_BAZEL_DISK_CACHE` environment
variable to an **absolute path** of a directory where you want to store the
cache.

For example, add the following line to your shell startup script
(e.g., `~/.bashrc` or `~/.zshrc`):

```bash
export FUCHSIA_BAZEL_DISK_CACHE="$HOME/.fuchsia_bazel_cache"
```

The build system will automatically create the directory if it does not exist.

Tip: Place the cache directory on the **same filesystem** as your Fuchsia
checkouts. This allows Bazel to use hard-links or reflinks (on supported
filesystems) to reference cached artifacts, which significantly reduces
disk space usage and improves build performance.

Important: The path specified in `FUCHSIA_BAZEL_DISK_CACHE` must be an absolute
path. Relative paths will cause the build to fail.

## Managing cache size (Garbage Collection) {:#managing-cache-size}

By default, the disk cache will grow indefinitely. To prevent it from
consuming too much disk space, you can enable automatic garbage collection by
setting the `FUCHSIA_BAZEL_DISK_CACHE_SIZE` environment variable.

This variable specifies the maximum size of the cache. Bazel will periodically
clean up older cached items in the background when it is idle to stay under
this limit.

Add the following line to your shell startup script, specifying the desired
limit (e.g., `40G` for 40 Gigabytes):

```bash
export FUCHSIA_BAZEL_DISK_CACHE_SIZE="40G"
```

The size can be specified in bytes, or optionally followed by `K`, `M`, `G`,
or `T`.

If `FUCHSIA_BAZEL_DISK_CACHE_SIZE` is not set, you must manage the cache size
manually (e.g., by deleting the directory contents when it grows too large).

## Avoiding caching for large artifacts {:#avoiding-caching-large-artifacts}

Caching is not always beneficial; for example, for targets that generate
large artifacts which change often, such as product assembly. Bazel provides
several ways to avoid caching these artifacts.

For individual target definitions, the
[`tags` attribute][tags-attribute]{:.external}, common to all rules, can be
used. For example:

```py
genrule(
    name = "create_large_archive",
    ...

    # No need to run this remotely, nor cache the archive.
    tags = ["no-cache", "no-remote"],
)
```

For custom rules, the implementation function should invoke
`ctx.actions.run()` with an `execution_requirements` attribute set to
a dictionary mapping tag strings to the string literal `"1"`. Note that using
`True` or the integer `1` will not work, which is not properly documented.
For example:

```py
def _my_rule_impl(ctx):
    ...
    ctx.actions.run(
        ...,
        execution_requirements = {
            "no-cache": "1",
            "no-remote": "1",
        },
    )
    ...

my_rule = rule(
    implementation = _my_rule_impl,
    ...
)
```

In this case, all `my_rule()` target artifacts will automatically not be
cached (or remoted).

[tags-attribute]: https://bazel.build/reference/be/common-definitions#common-attributes

