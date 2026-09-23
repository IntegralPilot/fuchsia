# Overview

`tiles-session` is a simple graphical session with basic window management functionality.

## Usage

### Build with `tiles-session`

Make sure `//src/ui/bin/tiles-session` to your gn build args, e.g:

```
fx set <product>.<board> --with //src/ui/bin/tiles-session
```

### Launch `tiles-session`

```
ffx session launch fuchsia-pkg://fuchsia.com/tiles-session#meta/tiles-session.cm
```

### Add your view

For example:
```
ffx session add fuchsia-pkg://fuchsia.com/flatland-examples#meta/flatland-rainbow.cm
```

## Current limitations
*TODO(https://fxbug.dev/42169911): update the following as features are added, and delete when fully-featured.*
Only one view is supported. Adding an additional view replaces the existing one.

# Use cases

`tile-sessions` fills a few roles in the Fuchsia ecosystem:

- educational: explain workings of a simple yet fully-functional session
- testing: reliable basis for integration tests
- development: support development by UI teams:
  - inspect
  - expose APIs for tests to query various conditions
