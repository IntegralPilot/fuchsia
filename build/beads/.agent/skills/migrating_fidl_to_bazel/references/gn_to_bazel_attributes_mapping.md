
Map the GN attributes to Bazel attributes as follows:
  - `sources` -> `srcs`
  - `public_deps` -> `deps`
  - `sdk_area` -> `api_area`
  - `sdk_category` -> `category`
  - `enable_* = true` -> `enable_* = True`
  - `contains_drivers = true` -> `contains_drivers = True`
  - `visibility = ["*"]` -> `visibility = ["//visibility:public"]`

For other values of `visibility`, map to the corresponding visibility in Bazel with reference to the following examples
Example:
  - Label `"//path/to/dir/*"` in BUILD.gn should be mapped to `"//path/to/dir:__subpackages__"` in the BUILD.bazel.
  - Label `"//path/to/dir:*"` in BUILD.gn should be mapped to `"//path/to/dir:__pkg__"` in the BUILD.bazel.
  - Label `"//path/to/dir:some_lib"` in BUILD.gn should be mapped to `"//path/to/dir:some_lib"` in the BUILD.bazel.
  - Label `"//path/to/dir"` in BUILD.gn should be mapped to `"//path/to/dir:dir"` in the BUILD.bazel.