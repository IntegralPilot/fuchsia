
A test helper that manages memory-backed blobfs instances.

## Usage

As this test helper is intended to be included within a test package, all tests
utilizing this helper will need a few extra blobs included in their test
package, and a decompressor sandbox component in the test realm.

### BUILD.gn

In each `fuchsia_test_package` that utilizes this crate, add dependencies on the
fxfs component and storage driver test realm. Add the `blobfs-corrupt` binary if
tests will want to corrupt blobs.

```
fuchsia_test_package("example-test-package") {
  deps = [
    "//src/storage/fxfs:fxfs_component",
    "//src/storage/testing:storage_driver_test_realm",
    "//src/storage/tools/blobfs-corrupt",
    ...
  ]
}
```

### Component Manifest

In the component manifest tests that utilize this crate, include the following shards:

```json5
{
    include: [
        "//src/storage/lib/fs_management/client.shard.cml",
        "//src/storage/testing/driver_test_realm/meta/client.shard.cml",
    ],
}
```
