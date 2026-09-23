# `shutdown-shim`

`shutdown-shim` is load-bearing piece of system-wide shutdown and reboot
procedures. All components that start a reboot or shutdown do so by talking to
`shutdown-shim` via `fuchsia.hardware.power.statecontrol.Admin`.

`shutdown-shim` does the following:

-   It implements `fuchsia.hardware.power.statecontrol.Admin`, allowing
    components to start a reboot or shutdown.
-   It alerts components of an impending reboot if they have registered their
    interest via
    `fuchsia.hardware.power.statecontrol.RebootMethodsWatcherRegister`.
-   It prevents suspension during the shutdown process.
-   It eliminates a dependency cycle between drivers and `power-manager` by
    implementing `fuchsia.system.state.SystemStateTransition`.
