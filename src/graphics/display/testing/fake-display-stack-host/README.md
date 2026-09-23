# Fake Display Stack Host

The `fake-display-stack-host` target in this directory defines a component
package of the same name. It hosts the [Fake Display Stack][fake-display-stack]
library in a testing [driver runtime][driver-runtime] and serves the
[`fuchsia.hardware.display.Service`] service that can be used for hermetic
testing without real hardware dependency to other components.

## Introduction

The goal of `fake-display-stack-host` is to provide a display engine
driver (`fake-display`) and a display coordinator driver within a hermetic
testing environment. This removes the dependency on acquiring the display
coordinator, allowing tests to run in display-less environments hermetically and
allowing multiple display coordinator and display driver instances to co-exist.

## Usage

### Define the child component statically in component manifest files

Realms should declare a child component in its component manifest for the
display coordinator connector. The child must be named
`fake-display-stack-host` to make its capabilities be correctly routed.

They should also include the corresponding shard component manifest file
`fake-display-stack-host.shard.cml` to route required capabilities to
the `fake-display-stack-host` child.

They should also explicitly offer the `fuchsia.hardware.display.Service`
service from `fake-display-stack-host` to clients (for example, Scenic).

For example, here is an excerpt of `ui_test_realm`'s component manifests
(`//src/ui/testing/ui_test_realm/meta/scenic.shard.cml`) declaring a fake
display coordinator connector from its own package, offering parent capabilities
to the display coordinator connector, and providing the
`fuchsia.hardware.display.Service` service to Scenic:

```json5
{
  include: [
    "//src/graphics/display/testing/fake-display-stack-host/meta/fake-display-stack-host.shard.cml",
  ],
  children: [
    {
      name: "fake-display-stack-host",
      url: "#meta/fake-display-stack-host.cm",
    },
  ],
  offer: [
    {
      service: "fuchsia.hardware.display.Service",
      from: "#fake-display-stack-host",
      to: ["#scenic"],
    },
  ],
}
```

### Create the child component dynamically using Realm Builder

For tests with multiple test cases, it's recommended to use [Realm Builder]
[realm-builder] to dynamically create a dedicated `fake-display-stack-host`
component instance for each test case.

An example of using Realm Builder to bring up `fake-display-stack-host`
is available in Flatland's [display compositor smoke test]
[flatland-display-compositor-smoketest].

In [`build-display-realm.cc`][build-display-realm], we define a fake display
realm that routes necessary services from the parent
to `fake-display-stack-host`, and routes the
[`fuchsia.hardware.display.Service]` service back to the test component.

```cpp
component_testing::RealmRoot BuildFakeDisplayRealm(async_dispatcher_t* dispatcher) {
  component_testing::RealmBuilder builder = component_testing::RealmBuilder::Create();
  static constexpr std::string_view kCoordinatorConnectorChildName = "fake-display-stack-host";
  builder.AddChild(std::string(kCoordinatorConnectorChildName), "#meta/fake-display-stack-host.cm");

  // Route capabilities from the test to the child component.
  builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Protocol{.name = "fuchsia.sysmem2.Allocator"},
                       component_testing::Protocol{.name = "fuchsia.tracing.provider.Registry"}},
      .source = component_testing::ParentRef(),
      .targets = {component_testing::ChildRef{.name = kCoordinatorConnectorChildName}},
  });

  // Route capabilities from the child component to the test.
  builder.AddRoute(component_testing::Route{
      .capabilities = {component_testing::Service{.name = "fuchsia.hardware.display.Service"}},
      .source = component_testing::ChildRef{.name = kCoordinatorConnectorChildName},
      .targets = {component_testing::ParentRef()},
  });
  return builder.Build(dispatcher);
}
```

Then, in the main test component, we can watch for the service to be provided
by the `fake-display-stack-host` realm.

```cpp
void SetUp() override {
  // [...]
  realm_root_ = testing::BuildFakeDisplayRealm(dispatcher());
  // [...]
  fidl::ClientEnd<fuchsia_io::Directory> svc_root(
      realm_root_->component().CloneExposedDir().TakeChannel());
  component::SyncServiceMemberWatcher<fuchsia_hardware_display::Service::Provider> watcher(
      svc_root.borrow());
  zx::result<fidl::ClientEnd<fuchsia_hardware_display::Provider>> provider_result =
      watcher.GetNextInstance(/*stop_at_idle=*/false);
  ASSERT_OK(provider_result);
  fidl::ClientEnd<fuchsia_hardware_display::Provider> provider =
      std::move(provider_result).value();
  // [...]
}
```

[build-display-realm]: /src/ui/scenic/lib/flatland/testing/build_display_realm.cc
[driver-runtime]: /docs/concepts/drivers/driver_framework.md#driver_runtime
[fake-display-stack]: /src/graphics/display/lib/fake-display-stack/
[flatland-display-compositor-smoketest]: /src/ui/scenic/lib/flatland/engine/tests/smoketests/display_compositor_smoketest.cc
[realm-builder]: /docs/development/testing/components/realm_builder.md
