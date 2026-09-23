# Overview

`tiling_wm` is a simple window management component for use with in-tree product sessions such as
`workbench_session`. It presents all views adjacent to one another, in a tiled, mosaic layout.
Views can be added to and removed from the session and focus transfers when the user interacts with
a new view.

## Usage

### Build with a `workbench_eng` product

Make sure to include in your gn build args the url to the view(s) you would like to add, e.g:

```
fx set workbench_eng.x64 --release --with //src/ui/examples:flatland-examples
```

### Add your view

Views can be added with `ffx session add <url> [--name <name>]`. Including `--name` is optional but
will be useful if you later want to remove the view. For example:

```
ffx session add fuchsia-pkg://fuchsia.com/flatland-examples#meta/simplest-app-flatland.cm --name simple_view
```

### Remove a view

Views can be removed by id using `ffx session remove <child_name>`. Each view has a unique `child_name`. If `--name` was specified when the view was added, then that will be its `child_name`. If you did not give the view a name when adding it, then you will have to find its identifier in `fx log`. When you initially add the view, a message will be logged, similar to this one:

```
[00336.146069][element_manager] INFO: launch_v2_element child_url=fuchsia-pkg://fuchsia.com/flatland-examples#meta/simplest-app-flatland.cm child_name=0lonupvr6iyxnzj8 collection=elements
```

Look for `child_name` (in this case `0lonupvr6iyxnzj8`) and use that id to remove the view.

# Use cases

`tiling_wm` fills a few roles in the Fuchsia ecosystem:

- educational: explain workings of a simple yet fully-functional window manager
- testing: reliable basis for integration tests
- development: support development by UI teams:
  - inspect
  - expose APIs for tests to query various conditions
