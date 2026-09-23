# Virtual device definitions for emulators

This directory contains the common virtual device configurations used
by `ffx emu` to create the virtual device that runs Fuchsia in the emulator.
These device configurations are included in product bundles. Product owners may
choose to include some or all of the standard definitions, and also to include
addition configurations.

See //build/assembly/product_bundle.gni to see how and which standard virtual devices
are added if no virtual devices are passed into the product_bundle template.
