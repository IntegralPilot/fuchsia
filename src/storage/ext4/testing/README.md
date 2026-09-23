# Testing

`test_ext4_server` is a utility component that can be used for testing of the
Ext4 library.

Note that this test server exposes an ext4 filesystem with full read
functionality but limited write support and any changes to metadata are not
persisted. Only truncating files to zero and overwriting file contents are
supported. Other operations will fail with a "not supported" error.


An ext4 image must be provided to the server. On startup, the server reads an
Ext4 filesystem image from its package data directory
(`/pkg/data/ext4_image.img`).

## Usage

To use this component in a test, use the `ext4_test_server` GN template provided
in `test_ext4_server.gni`.

```gn
import("//src/storage/ext4/testing/test_ext4_server.gni")

ext4_test_server("ext4_test_pkg") {
  image = "path/to/my_image.img"
}
```

This template will:
1.  Include the `my_image.img` as a resource in the package, mapped to
    `data/ext4_image.img`.
2.  Create a `fuchsia_package` containing the server and the image.

### Component Manifest Integration

To use the server in your test, include the provided manifest shard in your
test's component manifest. This shard automatically adds the server as a child
and mounts its `root` directory to `/test_ext4_filesystem_root`.

In `my_test.cml`:
```json5
{
    include: [
        "//src/storage/ext4/testing/meta/server.shard.cml",
    ],
    ...
}
```
