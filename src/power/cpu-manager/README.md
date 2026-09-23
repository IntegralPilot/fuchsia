The CPU Manager has a couple of distinct roles related to
CPU power usage and CPU thermal management.

## RPPM Policy Owner

The CPU Manager is a key part of Fuchsia's Runtime Processor
Power Management (RPPM). The CPU Manager is the userspace
policy component that is authorized to query and modify the
CPUs' operating performance points (OPP). It does this by
talking to the CPU driver via
fuchsia.hardware.cpu.ctrl.Device; the CPU driver can modify
the CPUs' frequencies and voltages.

As part of RPPM, the CPU Manager finds and loads an energy model
configuration and registers it with the kernel, via syscall.
It notifies the kernel's scheduler about changes to OPP, via syscall.
It also surfaces CPU stats in various ways.

The CPU Manager does not currently expose a FIDL to other components for
manipulating OPP.

## Thermal throttling of CPUs

The CPU Manager implements the CPU side of device thermal
throttling, by connecting to Power Manager's
fuchsia.thermal.ClientStateConnector and reducing CPU power
consumption when thermal load is exceeded.
