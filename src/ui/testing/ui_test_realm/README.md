# UI Test Realm

## Overview

The UI Test Realm is a library class to manage test realm on behalf of UI integration test clients.

## Test Realm

UITestRealm owns a RealmBuilder realm encapsulating the relevant portion of
the UI stack. The realm comprises two main parts:

(1) The UI layer component. This component runs the portion of the UI stack
specified by the client via the UITestRealm::Config argument passed
to the UITestRealm constructor. This portion of the realm is
(mostly) specified statically in [//src/ui/testing/ui_test_realm/meta/](./meta/).
(2) The client subrealm. This subrealm is a RealmBuilder Realm, owned and
configured by the client, containing any additional test-specific
components.

The component topology of the test is:

```
                        test_manager
                       /            \
         test fixture component      realm builder server
                   /
          ui test realm root
           /             \
   client subrealm     (ui layer components)
          |
    (test-specific
      components)
```

Clients can configure the scene owner, which specifies which UI layer
component to use (as specified statically in [//src/ui/testing/ui_test_realm/meta/](./meta/)). Clients can also specify the set of
UI services that must be routed to the client subrealm, and the set of client
services that must be exposed out of the top-level realm. UITestRealm will
configure all necessary routes between the UI layer component, the client
subrealm, and the top-level realm.

## Client Subrealm

The client can specify the test-specific portion of the component topology
within its own subrealm (see [`UITestRealm::AddSubrealm()`](./ui_test_realm.h)). Important notes on the client subrealm:

(1) A client subrealm should NOT contain any UI services (Scenic, Scene Manager, Text Manager, or A11y Manager).
(2) A client MUST expose `fuchsia.UI.app.ViewProvider` from its subrealm if it
specifies to `use_scene_owner`, thereby including Scene Manager.
(3) Clients can consume required UI services from ParentRef(), provided
they request those services in `Config::ui_to_client_services`.

## Input

UITestRealm supports input.

- If clients specify to use Scene Manager as the scene owner via `Config::use_scene_owner = true`, then UITestRealm assumes input pipeline (via Scene Manager) will own input for the test scene.
- If a client does not specify to use a scene owner, then UiTestRealm will expose raw scenic input APIs out of the test realm.

# Accessibility

UITestRealm enables configurations without accessibility, and also allows
clients to opt into using a Real or Fake A11y Manager. In general, clients
should not request accessibility unless it's explicitly required. For cases
where accessibility is required, clients should prefer using the Fake A11y
Manager for tests that require a11y services, but do not test a11y
functionality (e.g. tests that run a Chromium view). Clients should only use
a Real A11y Manager for tests that explicitly exercise accessibility-specific
behavior.
