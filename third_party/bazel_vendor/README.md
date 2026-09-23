# Bazel vendor directory

This directory is generated and maintained by tools. Avoid manually making
changes to content of this directory, unless you are certain they won't be
automatically overwritten in the future.

## To add a new Bazel dependency

1.  Add a new `--repo` argument in
    `//tools/devshell/contrib/update-bazel-vendor-dir`.
2.  Rerun `fx update-bazel-vendor-dir`.

## To upgrade a Bazel dependency

1.  Change the version of the dependency in
    `//build/bazel/toplevel.MODULE.bazel`.
2.  Run `fx update-bazel-vendor-dir`.

If new dependencies are required, follow the instructions above to vendor them.
