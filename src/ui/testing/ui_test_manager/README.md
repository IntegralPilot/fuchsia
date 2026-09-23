# UI Test Manager (DEPRECATED)

## Overview

The UI Test Manager is a [library class](./ui_test_manager.h) to manage test realm and scene setup on behalf of UI integration test clients.

**The UI Test Manager is deprecated; see https://fxbug.dev/42078308.**

## Test Realm

See [UI Test Realm](../ui_test_realm/README.md) for details.

## Scene Setup

UITestManager fulfills a similar role to session manager by bridging the
client view provider and the scene owner (scene manager).
Clients do NOT need to use the scene ownership APIs directly; instead, they
can rely on UITestManager to handle those details.

UITestManager also handles some of the scene setup synchronization on behalf
of clients. It allows clients to observe the "rendering" state of their test
view (i.e. the view created by the ViewProvider implementation the client
exposes from its subrealm). See `ClientViewIsRendering` for more details.

## Example Usage

```
// Configure UITestManager instance.
UITestRealm::Config config;
config.use_scene_owner = true;
config.ui_to_client_services = { fuchsia::ui::scenic::Scenic::Name_ };
UITestManager ui_test_manager(std::move(config));

// Add a client subrealm, and configure.
// This step must happen before calling BuildRealm().
auto client_subrealm = ui_test_manager.AddSubrealm();

// Add a view provider to the client subrealm.
client_subrealm.AddChild(kViewProvider, kViewProviderUrl);

// Expose the view provider service out of the subrealm.
client_subrealm.AddRoute(Route{.capabilities = {Protocol{fuchsia::ui::app::ViewProvider::Name_}},
                           .source = ChildRef{kViewProvider},
                           .targets = {ParentRef()}});

// Consume Scenic from the UI layer.
// UITestManager routes this service from the UI layer component to the client
// subrealm, so we consume it from ParentRef() within the subrealm.
client_subrealm.AddRoute(Route{.capabilities = {Protocol{fuchsia::ui::scenic::Scenic::Name_}},
                           .source = ParentRef(),
                           .targets = {ChildRef{kViewProvider}}});

// Build the realm, and take a copy of the exposed services directory.
ui_test_manager.BuildRealm();
auto realm_exposed_services = ui_test_manger.CloneExposedServicesDirectory();

// Create a test view, and attach it to the scene.
ui_test_manager.InitializeScene();

// Wait until the client view is rendering to proceed with the test case.
RunLoopUntil([&ui_test_manager](){
  return ui_test_manager.ClientViewIsRendering();
});

// Connect to some service in the test realm to drive the test.
auto service = realm_exposed_services.Connect<...>();
service->...;
```
