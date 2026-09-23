<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0244" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This RFC introduces the `zx_thread_raise_exception` syscall, which raises a
user-defined Zircon exception. The first use case for this syscall is for
Starnix to signal to the debugger when one of its processes calls `exec()`.
The debugger will use this signal to determine whether the developer wants to
attach to the process.

## Motivation

When running processes in Starnix, we often want to use the name of a process
to specify whether we want to attach the debugger to that process. This
approach works well if the process is already running because the debugger can
examine the `ZX_PROP_NAME` of the existing processes to find the one we want.
However, this approach does not work well for processes that are not yet
running because Starnix processes are created by `fork()`, at which time their
`ZX_PROP_NAME` matches the name of the process that called `fork()`. Starnix
changes the `ZX_PROP_NAME` of the process during `exec()`, but the debugger
never notices and therefore does not attach to the process.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

_Facilitator:_

The person appointed by FEC to shepherd this RFC through the RFC
process.

_Reviewers:_

- cpu@google.com
- jamesr@google.com
- jruthe@google.com

_Consulted:_

List people who should review the RFC, but whose approval is not required.


_Socialization:_

This problem was discussed in the Zircon chat channel, and I followed the
advice I received there to create a prototype, which demonstrates an end-to-end
flow for attaching to not-yet-running Starnix process by name using a
user-defined exception.

## Requirements

 1. The debugger must be notified when a Starnix process calls `exec()` so
    that it can check whether the process matches any of its filters (e.g.,
    whether the new name of the process matches a name filter).
 2. The notification mechanism should not be resource intensive if there is no
    debugger running.
 3. The notification mechanism must handle the case of multiple debug agents
    running simultaneously.
 4. The design should not require us to change other parts of the system (e.g.,
    `crashsvc`) that are not otherwise involved.

## Design

The debugger learns about new processes being created by listening for a
`ZX_EXCP_PROCESS_STARTING` exception on a
`ZX_EXCEPTION_CHANNEL_TYPE_JOB_DEBUGGER`. The approach in this RFC is to notify
the debugger of a process name change by sending another type of exception over
a `ZX_EXCEPTION_CHANNEL_TYPE_JOB_DEBUGGER`.

Unfortunately, we do not want Zircon to automatically generate an exception
when the `ZX_PROP_NAME` property of a process changes because that property can
be changed by an arbitrary thread. Instead, we wish for the exception to be
generated from a thread with the process whose name is changing. Fortunately,
Starnix always changes the name of a process from a thread within the process,
either via `exec()` or via a file in `procfs` that is only writable from within
a process.

For that reason, we introduce a new syscall for generating a user-defined
exception. Whenever Starnix changes the name of a process, Starnix will use
this syscall to raise such an exception. The debugger will listen for these
exceptions and re-scan its list of attach filters to see if the user wishes to
debug the process given its new name.

### User-defined exceptions

Similar to user-defined signals on Zircon objects, this RFC reserves part of
the Zircon exception namespace for user-defined exceptions. This reservation
ensures that user-defined exceptions will not conflict with future expansion of
system-defined exceptions.

Specifically, this RFC defines a new `zx_excp_type_t` with the `ZX_EXCP_SYNTH`
bit set called `ZX_EXCP_USER`:

```
#define ZX_EXCP_USER                    ((uint32_t) 0x309u | ZX_EXCP_SYNTH)
```

This RFC also defines a few well-known user exception codes, which appear in
the `synth_code` field in `zx_exception_context_t`.

```
ZX_EXCP_USER_CODE_PROCESS_NAME_CHANGED  ((uint32_t) 0x0001u)
ZX_EXCP_USER_CODE_USER0                 ((uint32_t) 0xF000u)
ZX_EXCP_USER_CODE_USER1                 ((uint32_t) 0xF001u)
ZX_EXCP_USER_CODE_USER2                 ((uint32_t) 0xF002u)
```

The `ZX_EXCP_USER_CODE_PROCESS_NAME_CHANGED` code will be used by Starnix and
the debugger for the use case described above. The `ZX_EXCP_USER_CODE_USER0`,
`ZX_EXCP_USER_CODE_USER1`, and `ZX_EXCP_USER_CODE_USER2` codes are defined for
application-specific uses, similar to `PA_USER0`, `PA_USER1`, and `PA_USER2`.

Codes less than `ZX_EXCP_USER_CODE_USER0` are reserved for system-wide uses,
and can be defined in later RFCs.

### Raising user-defined exceptions

This RFC defines a syscall for raising user-defined exceptions:

```
zx_status_t zx_thread_raise_exception(uint32_t options,
                                      zx_excp_type_t type,
                                      const zx_exception_context_t* context);
```

This syscall raises an exception of type `type` on the current thread with
the given exception context.

Currently, the `options` argument must be `ZX_EXCEPTION_JOB_DEBUGGER`, which
will have the value 1. If the caller passes any other value, the syscall
returns `ZX_ERR_INVALID_ARGS`. When this value is provided, the exception will
be delivered on the job debugger channel, if such a channel exists.

The `type` argument must be `ZX_EXCP_USER`. If the caller passes any other
value, the syscall returns `ZX_ERR_INVALID_ARGS`.

The `arch` field of `context` is ignored. The `synth_code` and `synth_data`
fields from the `context` are currently the primary mechanism to convey
information through the exception.

If we wish to extend this syscall to delivering exceptions to other types of
exception channels, we can expand the semantics of the syscall in a later RFC.

## Implementation

This feature will be implemented by adding the syscall described in the design
section. All of the machinery for raising the exception already exists in
Zircon. A second CL will teach the `debug_agent` to listen for these exceptions
and re-examine the name of the process that generated the exception.

A proof-of-concept CL has demonstrated that the implementation in Zircon and in
`debug_agent` is straightforward.

## Performance

This design has very little impact on system performance. In the common case of
there not being a running `debug_agent`, the `zx_thread_raise_exception` will
return early after walking to the root of the task hierarchy and noticing that
there is no one listening on a debugger exception channel.

Conversely, if there are many `debug_agent` instances running in the system,
this mechanism will deliver this notification to each of them efficiently.

This codepath has already been optimized for both these cases because the same
mechanism is used to notify `debug_agent` of other common events, such as
starting a process.

## Ergonomics

The approach described in this RFC is not particularly ergonomic.  For example,
Starnix needs to remember to raise the appropriate type of exception whenever it
changes the name of a process. A more ergonomic design would be for Zircon to
raise this exception automatically whenever the process name changes. However,
that approach is difficult because the process name can be changed from any
thread in the system that has a handle to the process. Zircon does not have a
mechanism to raise an exception from a remote thread, and adding such a
mechanism would add significant complexity to Zircon (e.g., to check for
pending exceptions before returning to userspace).

## Backwards Compatibility

The design described in this document is backwards compatible with the existing
system. The exceptions that can be generated by `zx_thread_raise_exception` are
clearly marked as user-generated exceptions and have a separate namespace from
kernel-generated exceptions. The design also reserves namespace for both future
user- and kernel-generated exceptions.

## Security considerations

The `zx_thread_raise_exception` provides a way for userspace to generate
exceptions that could not previously be generated, which an attacker could use
to manipulate software that is listening for exceptions. This proposal
mitigates that risk by constraining user-generated exceptions to having the
`ZX_EXCP_SYNTH` bit set and to a reserved namespace of even those exceptions.

Userspace can already generate some exceptions at the microarchitectural level,
for example using the `brk` and `int3` instructions on ARM and Intel,
respectively, which means the risk of adding a kernel-mediated mechanism for
generating exceptions is also reduced.

## Privacy considerations

Although process names could potentially contain privacy-sensitive information,
this new mechanism does not provide access to that information to any new
process. For example, this design has slightly better privacy properties than a
design that carried the new process name along with the exception.

## Testing

The new syscall will be tested by a Zircon core test.

The debugger integration will be tested with an integration test.

## Documentation

The new syscall will be documented with a syscall manual page, as usual for
Zircon system calls. The new exception semantics will also be documented in the
Exception Handling concepts page.

## Drawbacks, alternatives, and unknowns

We considered a number of alternatives:

### Use microarchitectural exception

Rather than adding a syscall to raise an exception, we could use the existing
microarchitectural mechanism for raising exceptions (e.g., the `brk` and `int3`
instructions). The downside of this approach is that these exceptions are fatal
unless handled. We could teach `crashsvc` to recognize these exceptions, as we
do for backtrace requests, but we would prefer not to teach `crashsvc` about
system functionality that is unrelated to `crashsvc`.

### Generate the exception automatically on process name change

Rather than requiring Starnix to call `zx_thread_raise_exception` after
changing the name of a process, we could have Zircon generate the exception
automatically whenever the name of a process changes. However, as discussed
above, the name of Zircon process can be changed by any thread that has a
handle to the process and Zircon lacks a mechanism for generating an exception
on a remote thread.

Fortunately, the only cases in which Starnix actually changes the name of a
process are ones in which the name change happens on a thread within that
process, which means we do not need to solve the problem of generating an
exception on a remote thread in order to address the use case at hand.
