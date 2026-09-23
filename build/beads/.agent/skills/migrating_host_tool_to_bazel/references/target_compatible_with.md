# target_compatible_with

If a target in the BUILD.gn file is guarded by `is_host`, (e.g.
`assert(is_host)` or `if (is_host)`), its equivalent target in the BUILD.bazel
file must specify the `target_compatible_with` attribute to:

**For tools not in the IDK:**
* Add `load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")` to `BUILD.bazel`.
* Set `target_compatible_with` to `HOST_CONSTRAINTS`.

**For tools in the IDK:**
* Add `load("//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")` to `BUILD.bazel`.
* Set `target_compatible_with` to ` HOST_OS_CONSTRAINTS`.

**NOTE:** In the BUILD.gn file, if the target is associated with a
`sdk_host_tool()` target or is defined using the template which invokes
`sdk_host_tool()` (e.g. `sdk_rustc_binary_host_tool()`) then it is a tool
included in IDK.


## Example - Tools Included in IDK
```gn
# BUILD.gn

...
sdk_host_tool("tool_in_idk") {
  ...
  deps = [ ":idk_tool_target" ]
}

go_binary("idk_tool_target") {
  ...
}

```

```bazel
# BUILD.bazel

...
load("//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")
load("//build/bazel/rules/idk:idk_host_tool.bzl", "idk_go_binary_host_tool")

package(default_applicable_licenses = ["//:license"])
...
idk_go_binary_host_tool(
  name = "idk_tool_target",
  target_compatible_with = HOST_OS_CONSTRAINTS,
  ...
)
```

## Example - Tools Not Included in IDK
```gn
# BUILD.gn

...

go_binary("tool_target") {
  ...
}

```

```bazel
# BUILD.bazel

...
load("@platforms//host:constraints.bzl", "HOST_CONSTRAINTS")
load("//build/bazel/rules/host:defs.bzl", "go_binary_host_tool")

package(default_applicable_licenses = ["//:license"])
...
go_binary_host_tool(
  name = "tool_target",
  target_compatible_with = HOST_CONSTRAINTS,
  ...
)
```

## Example - Libraries without Conditions
```gn
# BUILD.gn

...

go_library("my_lib") {
  ...
}

```

```bazel
# BUILD.bazel

...
load("@io_bazel_rules_go//go:def.bzl", "go_library")

package(default_applicable_licenses = ["//:license"])
...
go_library(
  name = "my_lib",
  # No `target_compatible_with required if the library is not guarded by `is_host` in GN.
  ...
)
```

## Example - Libraries guarded by `is_host`
```gn
# BUILD.gn

...
if (is_host) {
  go_library("my_host_lib") {
    ...
  }
}
...

```

```bazel
# BUILD.bazel

...
load("//build/bazel/platforms:constraints.bzl", "HOST_OS_CONSTRAINTS")
load("@io_bazel_rules_go//go:def.bzl", "go_library")

package(default_applicable_licenses = ["//:license"])
...
go_library(
  name = "my_host_lib",
  # Set `target_compatible_with` attribute if the library is guarded by `is_host`.
  target_compatible_with = HOST_OS_CONSTRAINTS,
  ...
)
```
