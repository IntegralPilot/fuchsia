# About

This library is used by drivers to provide outgoing protocols and services to other drivers and
components.

These can come in on two different transports:
 - Driver transport
 - Zircon transport

## Driver transport

This is a driver specific method of transport that all happens in process to avoid the overhead
of going into the kernel. This is used by colocated drivers (in the same driver host process)
for fast communication.

## Zircon transport

This is the generic transport used by all other Fuchsia components. The Zircon kernel facilitates
the communication.

# Why separate lib?

Drivers need to use this separate library, rather than existing component libraries for the
following reasons:

 - Globals not available per driver
 - Driver transport requires another library

## Globals not available per driver

The way elf components get access to their outgoing directory is through a numbered handle that is
passed by the elf runner. Before the main function is called, this is put into a global for access
through a libc function call.

Drivers could similarly store this in a global which is statically linked to the driver, but then
all instances of that driver within the driver host would use the same global,
and no shared libraries it links to would have access to that global. Therefore this option does
not fit out criteria.

Another option is by placing the global in a shared library of its own (fdio in our case) and then
having all parties link to that shared lib. This is exactly what non-driver elf components do.
However that would cause all drivers which link to that shared library to contain the same pointer
making the issue worse. So this option also does not fit our criteria.

Another option is to provide the libc and fdio methods through the driver runtime. This is because
the driver runtime can track the currently active driver. But this is only true in the ideal case
where Banjo doesn't exist, since Banjo calls can cross driver boundaries through function calls.
This makes the tracking in the driver runtime not completely accurate today. This option also does
not fit our criteria.

Therefore we simply want to avoid globals altogether. The outgoing directory is manually
provided using the driver's start arguments which is per driver instance.
The driver outgoing library wraps this outgoing directory into a class that the driver can use.
The interface for this is the exact same as the elf component variant, aside from the fact
that it supports multiple transports.

## Driver transport requires another library

This library supports both plain zircon transport and driver transport. To do this it needs to
bring in a separate library, including the entire driver runtime implementation, to provide driver
transport support. Non-driver components have no good reason for including this dependency, and
it will only bloat their runtime memory usage. Therefore this should be avoided.
