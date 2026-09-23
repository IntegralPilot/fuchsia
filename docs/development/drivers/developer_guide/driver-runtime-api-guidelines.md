# Driver runtime API guidelines

This page contains a set of principles and rules that Fuchsia follows when defining
C APIs in the driver runtime. Fuchsia is more opinionated for these APIs, compared to
others, because they are a significant part of the driver ABI.

## Naming

- APIs should follow the Zircon syscall naming convention of `<subject>_<verb>[_<object>]`.
  - In the API example of `fdf_dispatcher_get_options`, the subject is `dispatcher`,
    the verb is `get`, and  the object is `options`.
  - However, not all APIs have an object. In certain cases, the object may be an adverb
    (for instance, `fdf_channel_wait_async`).
  - Test-only and APIs for the embedding environment APIs refer to global singleton objects.
- APIs must start with the `fdf_` prefix.
  - Test-only APIs must start with the `fdf_testing_` prefix.
  - APIs meant for the embedding environment must start with the `fdf_env_` prefix.
- Output parameters must start with the `out_` prefix.

## Threading

- APIs should indicate whether they are blocking or not blocking in comment blocks.
- Blocking APIs must validate that they are invoked in the context of a driver runtime
  dispatcher, which has the `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS` option specified.
  - However, `zx_futex_wait` and `zx_futex_requeue` are exceptions to this rule because they
    cannot block on state external to the driver, which avoids risk of a deadlock outside
    of the control of the driver author.
- APIs must be thread-safe.
  - While they may return error when called in the incorrect dispatcher context, they should
    never act in an undefined manner.
- APIs that take in a callback should provide synchronous cancellation semantics[^1]
  in the context of a synchronized[^2] driver runtime dispatcher.
  - Asynchronous cancellation should result in the callback being called, possibly with an error
    status signaling forward progress was forced and the normal condition for triggering the
    callback was not met.
  - An API should never supply conditionally synchronous or asynchronous cancellation based on
    anything other than the type of dispatcher that the callback was registered against.

[^1]: Synchronous cancellation semantics mean that when the cancellation function call returns, it is guaranteed that the callback will no longer be called. Either it was already called and cannot be canceled, or was canceled successfully.

[^2]: A synchronized dispatcher is constructed via `fdf_dispatcher_create` when the `FDF_DISPATCHER_OPTION_UNSYNCHRONIZED` option is not specified.

## Objects

- APIs that create objects must have a corresponding API to destroy those objects.
- APIs that return created objects should return an opaque pointer to the object and not describe
  the internal structure of the object.
- APIs that create objects should take in a `uint32_t options` parameter with an extensible set
  of bit flags for extensibility.

## Parameters

- APIs should take in all non-primitive parameters by pointer.
- APIs that take in a client-allocated parameter that outlives the function call itself must do
  the following:
  - Take full ownership of the object, and any other objects it refers to if they are not
    self-referential (that is, also contained in the same allocation).
  - Supply a mechanism (such as a callback parameter) to return the object to the caller.
  - Provide a cancellation mechanism to proactively return the object to the caller before
    it would normally be returned.
  - Provide a mechanism to allow the client to use a single allocation to inline additional
    client-specific context after the object.
- Strings should not be assumed to be null terminated nor have “static” lifetimes.
- Callback parameters should be aliased to a new type by using `typedef`.
- Callback parameters should be embedded in a `struct` to ensure they can be stored in a single
  allocation.

## Miscellaneous

- APIs should have a comment block describing their purpose, parameters, outputs, and
  possible errors.
  - Examples are optional but recommended.
  - Comment blocks must use Markdown syntax, which is conducive to generating web-based
    documentation.
- APIs must be “memory tight” – in other words, memory allocated by the runtime must be
  freed by the runtime and memory allocated by the client must be freed by the client.
  -  An exception to this rule is allowed for arenas.
- APIs should prefer to return errors rather than assert if invoked incorrectly (for
  instance, on the wrong dispatcher with invalid parameters).
- Test-only APIs must be available only in the context of tests.
- Environment-only APIs must be available only to the environment in which the driver runs,
  not the driver itself.
