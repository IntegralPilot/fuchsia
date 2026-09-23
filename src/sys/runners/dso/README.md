# DSO Runner

A [component runner][runner] that runs components which are dynamic shared
object files (**DSO**). All components running in the same runner instance share
the same process and depending on the mode, either have their own thread or
share a thread. The main reasons for doing this are:

-   Reduce latency: components that share a thread can replace IPC with more
    efficient communication techniques, such as executor dispatch, FIDL driver
    transport, or local procedure call.
-   Save memory: Programs that share an address space can save some of the
    memory cost that is bound to a process.

This is similar to the strategy used by [Driver Runner][driver-runner] for
colocating device driver components, however DSO Runner's target audience is
non-driver platform components.

[driver-runner]: /docs/concepts/components/v2/driver_runner.md
[local-fidl]: /docs/development/languages/fidl/tutorials/cpp/topics/driver-transport.md
[runner]: /docs/concepts/components/v2/capabilities/runner.md
