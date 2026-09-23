---
name: driver-metadata-cpp-testing
description: >
  Test driver metadata in a C++ DFv2 driver. Use when a unit test must mock
  incoming metadata by serving it from a fdf_testing::Environment with
  fdf_metadata::MetadataServer, or verify outgoing metadata the driver serves
  by reading it back with fdf_metadata::GetMetadata over
  ConnectToDriverSvcDir(). For implementing metadata in the driver use the C++
  metadata implementation skill; to debug delivery failures use the metadata
  debugging skill.
---

# Driver Metadata Testing (C++) (DFv2)

## Dependencies

Add the following dependencies to the test's build target:

**GN:**
```gn
deps = [
  "//sdk/lib/driver/metadata/cpp", # For MetadataServer and GetMetadata
]
```

**Bazel:**
```bazel
deps = [
  "@fuchsia_sdk//pkg/driver_metadata_cpp", # For MetadataServer and GetMetadata
]
```

## Mock Incoming Metadata

When the driver calls
[`fdf_metadata::GetMetadata()`](/sdk/lib/driver/metadata/cpp/metadata.h) to
retrieve metadata from its parent, serve that metadata in the test environment.

Use
[`fdf_metadata::MetadataServer`](/sdk/lib/driver/metadata/cpp/metadata_server.h)
inside the `TestEnvironment` class.

```cpp
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/testing/cpp/driver_test.h>

class TestEnvironment : public fdf_testing::Environment {
 public:
  void SetMetadata(fuchsia_examples_metadata::Metadata metadata) {
    metadata_ = std::move(metadata);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    if (metadata_.has_value()) {
      async_dispatcher_t* dispatcher =
          fdf::Dispatcher::GetCurrent()->async_dispatcher();
      zx::result result = metadata_server_.Serve(
          to_driver_vfs, dispatcher, metadata_.value());
      if (result.is_error()) {
        return result.take_error();
      }
    }
    return zx::ok();
  }

 private:
  fdf_metadata::MetadataServer<fuchsia_examples_metadata::Metadata> metadata_server_;
  std::optional<fuchsia_examples_metadata::Metadata> metadata_;
};
```

In the test setup:
```cpp
TEST_F(MyDriverTest, TestWithMetadata) {
  // Initialize metadata in the environment
  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    env.SetMetadata(fuchsia_examples_metadata::Metadata({.test_property = "test value"}));
  });

  // Start the driver
  ASSERT_OK(driver_test().StartDriver());

  // Proceed with test assertions...
}
```

## Verify Outgoing Metadata

When the driver serves metadata to its children using
`fdf_metadata::MetadataServer`, verify that it served the correct data by
connecting to the driver's outgoing directory in the test.

```cpp
#include <lib/driver/metadata/cpp/metadata.h>

TEST_F(MyDriverTest, VerifyOutgoingMetadata) {
  // Start the driver
  ASSERT_OK(driver_test().StartDriver());

  // Connect to the driver's outgoing directory and retrieve the metadata
  zx::result<fuchsia_examples_metadata::Metadata> metadata =
      fdf_metadata::GetMetadata<fuchsia_examples_metadata::Metadata>(
          driver_test().ConnectToDriverSvcDir());

  ASSERT_OK(metadata);
  ASSERT_EQ(metadata.value().test_property(), "expected value");
}
```

## Common Pitfalls

* **Matching Types**: Ensure the template type used in
  `fdf_metadata::GetMetadata()` or `fdf_metadata::MetadataServer` matches the
  exact FIDL type defined in the implementation.

## Further Reading

* [/docs/development/drivers/tutorials/metadata-tutorial.md](/docs/development/drivers/tutorials/metadata-tutorial.md)
* [/sdk/lib/driver/metadata/](/sdk/lib/driver/metadata/) (Metadata library
  source)
* [Driver Metadata Implementation
  Skill](/src/devices/skills/driver_metadata/implementation/cpp/SKILL.md)
* [Driver Metadata Debugging
  Skill](/src/devices/skills/driver_metadata/debugging/SKILL.md)
* [Driver Metadata Examples](/examples/drivers/metadata/)
