# Basic Usage


## Taking action on system suspend or resume

A component may want to take some action when the CPU suspends or resumes. The
component can use an [`SuspendBlocker`][suspend_blocker] to observe these
transitions. The component registers by calling
[`fuchsia.power.system/ActivityGovernor.RegisterSuspendBlocker`][register_blocker].
The system will not suspend until after all blockers reply to
[`SuspendBlocker.BeforeSuspend`][before_suspend]. On system resume, System
Activity Governor will not raise the level of its `ApplicationActivity` element
until all blockers reply to [`SuspendBlocker.AfterResume`][after_resume].

[suspend_blocker]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.power.system/system.fidl;l=177;drc=93234a67c1f82cada0d5509dac7b76b32cef598c
[register_blocker]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.power.system/system.fidl;l=357;drc=93234a67c1f82cada0d5509dac7b76b32cef598c
[after_resume]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.power.system/system.fidl;l=211;drc=93234a67c1f82cada0d5509dac7b76b32cef598c
[before_suspend]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.power.system/system.fidl;l=199;drc=93234a67c1f82cada0d5509dac7b76b32cef598c