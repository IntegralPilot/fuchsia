# Virtual Keyboard Manager

This component provides virtual keyboard functionality.

## CML file and integration tests

The production package `//src/ui/bin/virtual_keyboard_manager`
includes `meta/virtual_keyboard_manager.cml`, which exists to serves routes
related to virtual keyboard input.

Generally, test packages should include their own copy of a component to ensure
hermeticity with respect to package loading semantics.

During regular maintenance, when adding a new service dependency, add it to
`meta/virtual_keyboard_manager.cml`, so that it is seen in both tests and production.
