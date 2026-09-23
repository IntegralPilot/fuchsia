# `component` shell tool

The `component` shell tool offers similar functionality as `ffx component` because it
reuses most of the code, but runs directly on the target fuchsia device.

The fuchsia shell on which `component` is running must have the following capabilities
available:
* /svc/fuchsia.sys2.RealmQuery.root
* /svc/fuchsia.sys2.RealmExplorer.root
* /svc/fuchsia.sys2.LifecycleController.root
* /svc/fuchsia.sys2.RouteValidator.root