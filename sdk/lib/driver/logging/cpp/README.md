# About

This library provides logging related features, like logging macros, to drivers.

# Why separate lib?

Drivers need to use this separate library, rather than existing logging libraries for the
following reasons:

 - Globals not available per driver

## Globals not available per driver

Elf components use the syslog library to create logs. This is a shared library that is shared by
everything in the process.

If driver components were to use the same library, then all logs that are emitted from a driver
host would go out of the same logger. This includes the driver host itself, as well as all drivers
sharing the host. Therefore this option does not fit our criteria.

Another option is to provide the syslog methods through the driver runtime. This is because
the driver runtime can track the currently active driver. But this is only true in the ideal case
where Banjo doesn't exist, since Banjo calls can cross driver boundaries through function calls.
This makes the tracking in the driver runtime not completely accurate today. This option also does
not fit our criteria.

Instead the route we have currently chosen is to use a global that is statically linked to just
the driver to store the logger. The downside with this is that logs coming from multiple instances
of the same driver in the host all go out of the same logger. We are ok with this tradeoff as it
simplifies the logging macros and avoids forcing the user to pass a logger in manually.
