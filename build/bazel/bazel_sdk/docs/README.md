# Documentation Generation

This directory contains targets which are used to create the docs for the
Fuchsia Bazel Rules.

If you need to add a new top-level bzl file it needs to be added to the
`@fuchsia_sdk//fuchsia:bzl_srcs` target. If you are just adding new interfaces
to an existing library then they will automatically be added to the list of
symbols that are exported.

If you need to add a new private directory to the source tree you will likely
need to add a new filegroup which exports the bzl files and any other deps
that are loaded. This library needs to be made visible to the //fuchsia:__pkg__
and added to the `@fuchsia_sdk//fuchsia:bzl_srcs` target.

```python
filegroup(
    name = "bzl_srcs",
    srcs = glob(["*.bzl"]) + [
        "@rules_license//rules:standard_package",
    ],
    visibility = ["//fuchsia:__pkg__"],
)
```
