# Expose the driver capabilities

Driver components offer the features and services they provide to other drivers
and non-driver components through [capabilities][concepts-capabilities].
This enables Fuchsia's component framework to route those capabilities to the
target component when necessary. For more details, see the
[Driver Communication][driver-communication] guide.

In this section, you'll expose the `qemu_edu` driver's factorial capabilities
and consume those from a component running elsewhere in the system.

## Create a new FIDL library

Create a new project directory in your Bazel workspace for a new FIDL library:

```posix-terminal
mkdir -p fuchsia-codelab/qemu_edu/fidl
```

After you complete this section, the project should have the following directory
structure:

```none {:.devsite-disable-click-to-copy}
//fuchsia-codelab/qemu_edu/fidl
                  |- BUILD.bazel
                  |- qemu_edu.fidl
```

Create the `qemu_edu/fidl/BUILD.bazel` file and add the following statement to
include the necessary build rules from the Fuchsia SDK:

`qemu_edu/fidl/BUILD.bazel`:

```bazel
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/fidl/BUILD.bazel" region_tag="imports" adjust_indentation="auto" %}
```

## Define a device service

The driver exposes the capabilities of the `edu` device using a custom FIDL
service. This service contains one or more protocols that clients can connect
to. Add a new `qemu_edu/qemu_edu.fidl` file to your project workspace with
the following contents:

`qemu_edu/fidl/qemu_edu.fidl`:

```fidl
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/fidl/qemu_edu.fidl" region_tag="example_snippet" adjust_indentation="auto" %}

```

This FIDL file defines the `Device` protocol with two methods, and the
`DeviceService` which makes the `Device` protocol available to clients.

Add the following build rule to the bottom of the project's build configuration
to compile the FIDL library and generate C++ bindings:

*   `fuchsia_fidl_library()`: Declares the `examples.qemuedu` FIDL
    library and describes the FIDL source files it includes.

`qemu_edu/fidl/BUILD.bazel`:

```bazel
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/fidl/BUILD.bazel" region_tag="fidl_library" adjust_indentation="auto" %}
```

## Implement the device service

With the FIDL service defined, you'll need to update the driver to implement
the `Device` protocol and serve the `DeviceService` to other components.

After you complete this section, the project should have the following directory
structure:

```none {:.devsite-disable-click-to-copy}
//fuchsia-codelab/qemu_edu/drivers
                  |- BUILD.bazel
                  |- meta
                  |   |- qemu_edu.cml
                  |- edu_device.cc
                  |- edu_device.h
{{ '<strong>' }}                  |- edu_server.cc {{ '</strong>' }}
{{ '<strong>' }}                  |- edu_server.h {{ '</strong>' }}
                  |- qemu_edu.bind
                  |- qemu_edu.cc
                  |- qemu_edu.h
```

Create the new `qemu_edu/drivers/edu_server.h` file in your project directory
with the following contents to include the FIDL bindings for the
`examples.qemuedu` library and create a new `QemuEduServer` class to
implement the server end of the `Device` protocol:

`qemu_edu/drivers/edu_server.h`:

```cpp
#ifndef FUCHSIA_CODELAB_QEMU_EDU_SERVER_H_
#define FUCHSIA_CODELAB_QEMU_EDU_SERVER_H_

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.h" region_tag="imports" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.h" region_tag="namespace_start" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.h" region_tag="fidl_server" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.h" region_tag="namespace_end" adjust_indentation="auto" %}

#endif  // FUCHSIA_CODELAB_QEMU_EDU_SERVER_H_

```

Create the new `qemu_edu/drivers/edu_server.cc` file in your project directory
with the following contents to implement the `examples.qemuedu/Device`
protocol methods and map them to the device resource methods:

`qemu_edu/drivers/edu_server.cc`:

```cpp
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.cc" region_tag="imports" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.cc" region_tag="namespace_start" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.cc" region_tag="compute_factorial" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.cc" region_tag="liveness_check" adjust_indentation="auto" %}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/edu_server.cc" region_tag="namespace_end" adjust_indentation="auto" %}

```

Update the driver's build configuration to depend on the FIDL bindings for the
new `examples.qemuedu` library:

`qemu_edu/drivers/BUILD.bazel`:

{% set build_bazel_snippet %}
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/BUILD.bazel" region_tag="binary" adjust_indentation="auto" exclude_regexp="fuchsia\.device\.fs" highlight="13" %}
{% endset %}

```bazel
{{ build_bazel_snippet|replace("//src/qemu_edu","//fuchsia-codelab/qemu_edu")|trim() }}
```

## Export and serve the service

The `qemu_edu` driver makes the `fuchsia.examples.qemuedu.DeviceService`
discoverable to other components by serving it from its outgoing directory.
To do this, the driver must:

 - Declare this service as a `capability` in its component manifest.
 - `expose` this capability to the component framework in its component manifest.

This allows other components, including non-driver components, to request the
service by its type without needing to know the topological path of the
driver. The component framework is responsible for routing the service connection
from the client to the driver that provides it.

Update the driver's component manifest to declare and expose the device service
as a capability:

`qemu_edu/drivers/meta/qemu_edu.cml`:

```json5
{
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/meta/qemu_edu.cml" region_tag="driver" %}
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/meta/qemu_edu.cml" region_tag="use_capabilities" exclude_regexp="protocol" %}
{{ '<strong>' }}{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/meta/qemu_edu.cml" region_tag="expose_capabilities" %}{{ '</strong>' }}
}
```

With the manifest updated, the component framework can now route requests for
this service. The final step is for the driver to implement the server for the
service and serve it on its outgoing directory.

Update the driver's `Start()` method to begin serving the `fuchsia.examples.qemuedu.DeviceService`
from the component's outgoing directory:

`qemu_edu/drivers/qemu_edu.cc`:

```cpp
{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/qemu_edu.cc" region_tag="imports" adjust_indentation="auto" %}

{{ '<strong>' }}{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/qemu_edu.cc" region_tag="fidl_imports" adjust_indentation="auto" %}{{ '</strong>' }}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/qemu_edu.cc" region_tag="start_method_start" adjust_indentation="auto" %}
  // ...

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/qemu_edu.cc" region_tag="device_registers" %}

{{ '<strong>' }}{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/qemu_edu.cc" region_tag="serve_outgoing" %}{{ '</strong>' }}

{% includecode gerrit_repo="fuchsia/sdk-samples/drivers" gerrit_path="src/qemu_edu/drivers/qemu_edu.cc" region_tag="start_method_end" adjust_indentation="auto" %}
```

The `qemu_edu` driver's capabilities are now discoverable by other components.

## Rebuild the driver

Use the `bazel build` command to verify that the driver builds successfully with
your code changes:

```posix-terminal
bazel build //fuchsia-codelab/qemu_edu/drivers:pkg
```

Congratulations! You've successfully exposed FIDL services from a Fuchsia driver.

<!-- Reference links -->

[concepts-capabilities]: /docs/concepts/components/v2/capabilities/README.md
[driver-communication]: /docs/concepts/drivers/driver_communication.md
[fidl-cpp-bindings]: /docs/reference/fidl/bindings/cpp-bindings.md