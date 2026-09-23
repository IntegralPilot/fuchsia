<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0240" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This proposes that asynchronous operations in the Zircon kernel are performed on
kernel objects and are not tied to the handle used to register those operations.

## Motivation

Zircon defines asynchronous waiting operations on kernel objects. We would like
to add asynchronous operations that have side effects such as asynchronous
channel reads. In order to do so, we need to nail down the semantics of these
operations and their interactions with other operations on the handles and/or
objects involved.

object_wait_async is an example of an asynchronous operation that generates a
port packet when an object's state changes. The current API tries to split the
difference between the operations being performed on the underlying object - as
the name would suggest - and the handle identifying the operation.

## Stakeholders

Zircon

_Facilitator:_

The person appointed by FEC to shepherd this RFC through the RFC
process.

_Reviewers:_

- maniscalco@google.com
- cpu@google.com
- abarth@google.com

_Consulted:_


_Socialization:_


## Requirements

Define the relationship of asynchronous kernel operations including
object_wait_async and handle interactions such as transferring and closing
handles.

## Design

### Overview

Asynchronous operations in Zircon operate on objects (not handles). Changes in
the object state may influence these operations. Changes in the handles used to
refer to the objects do not influence the behavior of asynchronous operations,
with a carveout for channels specifically described below. Once registered, an
asynchronous operation continues until it completes, is canceled explicitly, or
all handles to the object are closed. The facility for canceling asynchronous
operations on a port is updated to reflect the new semantics.


### Syscall changes

`object_wait_async`:

- Remove the sentence "If handle is closed, the operations associated with it
will also be terminated, but packets already in the queue are not affected."
from the documentation and backing code.

Once registered, an asynchronous wait remains valid until the object state
changes to match the observed condition, the wait is explicitly canceled, or
all handles to the object are closed.

`port_cancel`:

- Deprecate `port_cancel` and provide `port_cancel_key` as a replacement that
cancels all operations on the specified port using the specified key. Signature:

```
zx_port_cancel_key(zx_handle_t port, uint32_t options, uint64_t key)
```

This is also an opportunity to add an `options` parameter which `port_cancel`
currently lacks.

An alternative would be to update the implementation of `port_cancel` to ignore
the `source` parameter. In practice cancelable port operations are registered
with a unique key per operation. Typically this key is the address of a data
structure allocated to handle the operation itself. There is evidence to suggest
that this change would be safe (see the "Backwards Compatibility" section) but
it would be challenging to deploy this change safely and reclaim the `options`
slot while using the same name. Using a new name for the operation will allow
us to update code incrementally and track callers of the old operation.

`object_wait_one` and `object_wait_many` with `handle_close`:

- No change in explicitly documented feature set. Document that closing
a handle during a synchronous wait is deprecated.

These are synchronous waits on one or more objects identified by handle. We
currently document that closing a handle cancels any currently pending waits
through these operations, but this is inherently racy since there is no way for
one thread closing a handle to know if another thread calling
`object_wait_{one,many}` has resolved that handle yet. A program racing
`handle_close` and `object_wait_{one,many}` against each other could observe
either `ZX_ERR_CANCELED` or `ZX_ERR_BAD_HANDLE`. We can provide a race-free way
to cancel these synchronous wait operations by dissociating the handle from the
object (https://fxrev.dev/949517) or by making it easier for programs to wait
for multiple conditions so that they can synchronize access to the handle in
userspace. Since it is currently ambiguous when in the operation the handle
needs to be valid, this proposal does not change the behavior.

`channel_call` and `channel_call_etc`:

- No change in explicitly documented feature set.

These currently have implicit documentation of the same cancelation behavior as
`object_wait_{one,many}` in the `ZX_ERR_CANCELED` return value. They also have
the same sort of race issue as those operations when using `handle_cancel` in
the general case, although since this operation exposes more internal details it
is possible to concoct scenarios where close will not race in practice. The
internal wait of `channel_call` cannot be parameterized by callers beyond a
timeout so programs cannot use multiple conditions to coordinate shutdown. To
provide race-free cancelation we'll need to define a separate operation such as
the one proposed in https://fxrev.dev/949517. As with `object_wait_{one,many}`
while the current behavior is suspect this RFC does not propose any changes to
the behavior of these operations.


### Channel ordering

Channels serve a unique role in the Fuchsia system architecture and currently
have special case logic in the kernel to verify that operations on a channel
handle only succeed when the handle is owned by the calling process. Channels
are used extensively in the system to exchange data and capabilities. The
purpose of these checks is to maintain confidentiality and integrity on channels
when handles are passed between processes operating at different trust levels.
Once a process gains possession of a handle to a channel it is promised
exclusive access to read and write messages from that endpoint. To preserve this
property with the semantics proposed in this RFC and allowing for future
asynchronous channel operations we can take advantage of another property of
channels which is that channel endpoints have exactly one handle. A transfer
operation on a handle to a channel can be considered an operation on the object
itself and we can either cause pending mutation operations on the channel or the
transfer attempt to fail.

Observation of object state such as the READABLE and WRITABLE signals does not
violate properties that we care about. It is acceptable if a program can observe
when a channel is readable or writable even after transferring this handle away
as this information does not carry capabilities or important confidential
information about the state of the system. This means that the
`object_wait_async` operation does not need to behave differently for channels
with this proposal. Future operations on channels such as asynchronous reads
will need to consider interactions with transfer.

## Implementation

The main practical impact of this change on userspace is on cancelation of
asynchronous object waits on port objects. Asynchronous dispatcher libraries
will need to allocate and track unique keys for cancelable operations, which
they already do, and will no longer need to track handle values if they are
not using the objects. Calls to `port_cancel` will be translated mechanically
to `port_cancel_key` by changing the second parameter from a handle value to
the literal `0`.

On the kernel side, the relationship between observers, ports, and the object
representation is very subtle. This change will alleviate some complexity by
removing a dependency on the handle table lock during the cancelation
procedure and enable future refactors of the handle managing logic. In the
short term, the most straightforward way to implement this change is to
register asynchronous waits with a null `Handle` and perform asynchronous
wait cancelation by marking the registered waits as canceled and cleaning
up the observer state when the observer fires.

## Performance

This change reduces the amount of kernel bookkeping on asynchronous operations
and the stress on the handle table lock which may help under certain types of
load. Our current microbenchmarks show no significant change with a simple
prototype implementation. Userspace libraries can also store less information
in order to cancel a registered asynchronous operation.

## Ergonomics

This requires applications track key values if they wish to cancel individual
asynchronous operations.  In practice dispatcher implementations do this already
(in addition to keeping a handle value).

## Backwards Compatibility

This changes the documented and actual semantics for `object_wait_async` and
`port_cancel` in subtle ways. The changes do not impact programs that maintain
these properties:

- Asynchronous waits are registered with unique keys per wait
- Handles to objects being waited on are not closed or transferred with
registered waits still pending

These are generally true today. Asynchronous waits are usually associated with
an allocation of state to handle the result of the wait and the `key` for these
waits is usually computed based on the address of this allocation which is
necessarily unique within the address space or upon another form of unique
identifier. Handles are usually stored in owning data structures and closing the
handle requires going through some sort of shutdown procedure that clears
references to it.

https://fxrev.dev/984701 is a prototype that ignores the `source` parameter to
`port_cancel`. There is one Zircon core test that reuses keys intentionally,
all other test cases in the tree pass unmodified. This suggests that the
changes to `port_cancel` are compatible with current code.

https://fxrev.dev/986494 is a related prototype that decouples the lifetime
of a registered handle with the observer. This evaluates whether we rely on
closing a handle to cancel outstanding asynchronous waits. All of our existing
tests pass unmodified with this change suggesting that this change in behavior
is compatible with current code.


## Security considerations

This proposal allows processes to observe changes in object state that they no
longer have a handle to in some situations. In this model, a handle represents
the capability to initiate an asynchronous operation. We do not in general
promise that a handle represents exclusive access to an object outside the
special case of objects with non-duplicatable handles (channels, most notably).

For most object types that grant duplicatible handles a process that possesses a
handle cannot reason about what other handles and operations on that handle
exist unless it has knowledge of the full history of that handle back to
creation or transfer from a trusted source. This analysis does not change
significantly if we allow asynchronous operations to persist on an object after
a handle to it is transferred as the registrar could just as easily have
retained a duplicate handle.

For channels specifically as we always grant non-duplicatible handles we do
promise exclusive access to a process possessing a handle to a channel. Allowing
a previous owner of a channel handle to observe when a channel first becomes
readable after transferring it away is likely not security sensitive. We'll need
to be careful when introducing operations that interact with the contents of
messages sent through channels such that only current handle owners are
permitted to perform these operations.

## Testing

The changes to port and asynchronous object behavior will be tested with
Zircon's core test suite. Library compatibility - particularly in dispatcher
libraries - will be evaluated through inspection of the libraries and their test
suites. Our existing integration suites pass with a prototype of the new
behavior (see the "Backwards Compatibility" section) and we can monitor these
closely when deploying the change to `object_wait_async`.

## Documentation

System call documentation will need to be added for the new `port_cancel_key`
and documentation for existing operations will need to be updated as described
in the design section.

## Drawbacks, alternatives, and unknowns

### Alternative semantics based around handles

One alternative is to continue to tie asynchronous operations to their
initiating handle (as some of the current documentation suggests) and maintain
this relationship as we add new asynchronous operations. This will add
complexity to these new operations and make it more difficult to refactor and
optimize the kernel's handle managing logic while providing little utility to
application logic.

### `port_cancel` reuse

Instead of defining a new `port_cancel_key` syscall we could replace the
`source` parameter on the existing `port_cancel` with an `options` parameter of
the same size.  For a transition period, no options would be supported and
handle values would also be permitted in this field. During this transition
period we would find and update all callers to `port_cancel` to pass a 0 value
in this field instead of the handle value they currently supply. At the end of
the transition period we would start enforcing that the field be zero, at which
point we could start introducing option values. A challenge with this approach
is that it may be difficult to detect when all callers are updated to the new
semantics. It is rare in practice to cancel an asynchronous wait before it fires
and so dynamic analysis of `port_cancel` may not find callers that are only
reached in unusual error handling or timing situations.

## Prior art and references

[Zircon handle resolution model](https://fxrev.dev/455194) - draft RFC from 2020
exploring some of these issues.

[zx_handle_cancel](https://fxrev.dev/949517) - draft RFC defining a mechanism
for canceling synchronous operations on a handle avoiding handle resolution
races.

[async-loop key management](https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/async-loop/loop.c;l=589;drc=36e68808ae97fc54675be5c3f57484726577425c)

[zxwait key management](https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/go/src/syscall/zx/zxwait/zxwait.go;l=72;drc=ae86e92e4a96a51b1de243443c4fbe744f8c7740)

[fuchsia-async key management](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/fuchsia-async/src/runtime/fuchsia/executor/packets.rs;l=83;drc=9cbf3869e15f4ba7e445ac18e69e4b969046ca2b)