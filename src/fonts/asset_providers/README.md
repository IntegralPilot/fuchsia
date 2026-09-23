# Asset Providers

This directory contains the definitions for font asset providers.

These components are built out of:

- A font server component (the server binary)
- An asset provider component (a component with font assets)
- A font provider component (the top-level component which routes to the above
    two)

and the appropriate capability routing.

See the documentation on //src/fonts/build/font_asset_provider.gni for usage.

