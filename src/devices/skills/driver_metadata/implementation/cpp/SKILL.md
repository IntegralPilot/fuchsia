---
name: driver-metadata-cpp-impl
description: >
  Implement driver metadata in a C++ DFv2 driver -- send, retrieve, or forward
  a @serializable FIDL-table metadata type. Use when a C++ driver must serve
  metadata to children with fdf_metadata::MetadataServer::Serve and pass
  CreateOffer() to AddChild, read parent metadata with
  fdf_metadata::GetMetadata, forward it with ForwardAndServe, or declare the
  metadata service (named for the FIDL type) in capabilities/expose/use. For
  writing metadata tests use the C++ metadata testing skill; to debug delivery
  failures use the metadata debugging skill.
---

# Driver Metadata Implementation (C++) (DFv2)

## Dependencies

Add the following dependencies to the driver's build target:

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

## Implementation Steps

### 1. Define Metadata in FIDL

The metadata type must be defined as a FIDL type and annotated with
`@serializable`.

**Where to define it**: The type should be defined in a FIDL library that is
accessible to both the sender and receiver drivers. Typically, this is the
library that defines the device's primary protocol or a dedicated library for
that hardware subsystem (e.g., `fuchsia.hardware.nand`).

```fidl
library fuchsia.examples.metadata;

@serializable
type Metadata = table {
    1: test_property string:MAX;
};
```

### 2. Send Metadata

To send metadata to child drivers, use `fdf_metadata::MetadataServer`.

Include the header
[metadata_server.h](/sdk/lib/driver/metadata/cpp/metadata_server.h):
```cpp
#include <lib/driver/metadata/cpp/metadata_server.h>
```

In the driver class, declare a `MetadataServer`:
```cpp
#include <lib/driver/component/cpp/driver_base2.h>

class MyDriver : public fdf::DriverBase2 {
 public:
  MyDriver() : DriverBase2("my-driver") {}

 private:
  fdf_metadata::MetadataServer<fuchsia_examples_metadata::Metadata> metadata_server_;
};
```

In the `fdf::DriverBase2::Start()` method, serve the metadata and pass the offer
to the child node:
```cpp
// Set and serve the metadata
fuchsia_examples_metadata::Metadata metadata({.test_property = "test value"});
auto serve_status = metadata_server_.Serve(*outgoing(), dispatcher(), metadata);
if (serve_status.is_error()) {
    return serve_status.take_error();
}

// Create an offer for the child node
std::vector<fuchsia_driver_framework::Offer> offers;
std::optional<fuchsia_driver_framework::Offer> metadata_offer = metadata_server_.CreateOffer();
if (metadata_offer.has_value()) {
    offers.push_back(metadata_offer.value());
}

// Pass the offers to AddChild
zx::result child = AddChild("child", {}, offers);
```

#### Component Manifest (.cml) Update

Declare and expose the service in the driver's `.cml` file. The service name is
the fully qualified FIDL type name.

```cml
    capabilities: [
        { service: "fuchsia.examples.metadata.Metadata" },
    ],
    expose: [
        {
            service: "fuchsia.examples.metadata.Metadata",
            from: "self",
        },
    ],
```

### 3. Retrieve Metadata

To retrieve metadata from a parent driver, use `fdf_metadata::GetMetadata`.

Include the header [metadata.h](/sdk/lib/driver/metadata/cpp/metadata.h):
```cpp
#include <lib/driver/metadata/cpp/metadata.h>
```

In the `fdf::DriverBase2::Start()` method, use the incoming namespace from the
context to retrieve metadata:

```cpp
zx::result<> Start(fdf::DriverContext context) override {
  zx::result<fuchsia_examples_metadata::Metadata> metadata =
      fdf_metadata::GetMetadata<fuchsia_examples_metadata::Metadata>(
          context.svc());
  if (metadata.is_error()) {
      fdf::error("Failed to get metadata: {}", metadata.status_string());
      return metadata.take_error();
  }
  // Use metadata.value()

  return zx::ok();
}
```

#### Component Manifest (.cml) Update

Declare that the driver uses the service in its `.cml` file:

```cml
    use: [
        { service: "fuchsia.examples.metadata.Metadata" },
    ],
```

### 4. Forward Metadata

To retrieve metadata from a parent and forward it to a child:

```cpp
zx::result is_serving =
    metadata_server_.ForwardAndServe(*outgoing(), dispatcher(), context.svc());
if (is_serving.is_error()) {
    return is_serving.take_error();
}

std::vector<fuchsia_driver_framework::Offer> offers;
std::optional<fuchsia_driver_framework::Offer> metadata_offer =
    metadata_server_.CreateOffer();
if (metadata_offer.has_value()) {
    offers.push_back(metadata_offer.value());
}

zx::result child = AddChild("child", {}, offers);
```

#### Component Manifest (.cml) Update

Update the manifest to `use` and `expose` the service:

```cml
    use: [
        { service: "fuchsia.examples.metadata.Metadata" },
    ],
    capabilities: [
        { service: "fuchsia.examples.metadata.Metadata" },
    ],
    expose: [
        {
            service: "fuchsia.examples.metadata.Metadata",
            from: "self",
        },
    ],
```

## Common Pitfalls

* **Service Naming**: The service name in the manifest must match the FIDL type
  name, not `fuchsia.driver.metadata.Service`.
* **Missing Offer**: Forgetting to call `CreateOffer()` and pass it to
  `AddChild` will prevent the child from accessing the metadata.
* **Using Structs instead of Tables**: Using a FIDL `struct` for metadata makes
  it difficult to evolve the data structure in the future without breaking
  compatibility. Prefer using a FIDL `table` with optional fields to allow for
  safe additions or removals of fields over time.

## Further Reading

* [/docs/development/drivers/tutorials/metadata-tutorial.md](/docs/development/drivers/tutorials/metadata-tutorial.md)
* [/sdk/lib/driver/metadata/](/sdk/lib/driver/metadata/) (Metadata library
  source)
* [Driver Metadata Testing
  Skill](/src/devices/skills/driver_metadata/testing/cpp/SKILL.md)
* [Driver Metadata Debugging
  Skill](/src/devices/skills/driver_metadata/debugging/SKILL.md)
* [Driver Metadata Examples](/examples/drivers/metadata/)
