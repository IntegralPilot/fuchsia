# Tracing asynchronously

This guide shows how to add asynchronous tracing to your code.

Note: For more information on the Fuchsia tracing system, see
[Fuchsia tracing system](/docs/concepts/kernel/tracing-system.md).

## Prerequisites

Before you begin, make sure you have completed the following:

* [Familiarized yourself with the Fuchsia tracing system](/docs/concepts/kernel/tracing-system.md).
* [Registered your component as a tracing provider](/docs/development/tracing/tutorial/register-a-trace-provider.md).
* [Familiarized yourself with adding synchronous tracing your code](/docs/development/tracing/tutorial/add-tracing-in-code.md).

Also, make sure to use the relevant library in your code:

<<../_common/_tracing_headers.md>>

## Add asynchronous tracing

There's a set of asynchronous tracing functions that are used when the
operation spans multiple threads.

For example, in a multi-threaded server, a request is handled by one thread,
and then put back on a queue while the operation is in progress.
Some time later, another thread receives notification that the operation has
completed, and "picks up" the processing of that request.
The goal of asynchronous tracing is to allow the correlation of these disjoint
trace events.

Asynchronous tracing takes into consideration that the same code path is used
for multiple different flows of processing.
In the previous examples, we were interested in seeing how long a particular
function ran, or what a certain value was at a given point in time.
With asynchronous tracing, we're interested in tracking the same data, but for
a logical processing flow, rather than a program location based flow.

In the queue processing example, the code that receives requests would tag each
request with a "nonce" &mdash; a unique value that follows the request around.
This nonce can be generated with `TRACE_NONCE()`, which simply increments a
global counter.

Let's see how this works.
First, you declare a place to hold the nonce.
This is usually in a context structure for the request itself:

```c
typedef struct {
...
    // add the nonce to your context structure
    trace_async_id_t async_id;
} my_request_context_t;
```

When the request arrives, you fetch a nonce and begin the asynchronous tracing
flow:

```c
// a new request; start asynchronous tracing
ctx->async_id = TRACE_NONCE();
TRACE_ASYNC_BEGIN("category", "name", ctx->async_id, "key", TA_STRING("value"));
```

You can log trace events periodically using the `TRACE_ASYNC_INSTANT()` macro
(similar to what we did with the `TRACE_INSTANT()` macro above):

```c
TRACE_ASYNC_INSTANT("category", "name", ctx->async_id, "state", TA_STRING("phase2"));
```

And clean up via `TRACE_ASYNC_END()`:

```c
TRACE_ASYNC_END("category", "name", ctx->async_id);
```

> Don't confuse this use of "async" with the async loop that's running in your
> process; they aren't related.

## Flow tracing

Asynchronous tracing is intended for tracing within the same process, but
perhaps by way of different threads.

There's a higher-level tracing mechanism, called "flow" tracing, that's
intended for use between processes or abstraction layers.

You call `TRACE_FLOW_BEGIN()` to mark the start of a "flow".
To identify the flow, you must pass in a globally unique flow ID.

Important: Flow IDs are global across the entire trace and are not scoped by
flow name or category. Using duplicate IDs will cause visual bugs in trace
viewers like Perfetto. Always use unique IDs (e.g., generated via
`TRACE_NONCE` or `TRACE_RANDOM_ID` in C/C++ or `fuchsia_trace::Id::new()`
in Rust).

Also note that Perfetto currently drops flow names and arguments. If you
need to track arguments, attach them to the enclosing duration events.

Then, you (optionally) call `TRACE_FLOW_STEP()` to indicate
trace operations within that flow.

When you're done, you end the flow with `TRACE_FLOW_END()`.

A flow could be used, for example, between a client and server for tracking a
request end-to-end from the client, through the server, and back to the client.

