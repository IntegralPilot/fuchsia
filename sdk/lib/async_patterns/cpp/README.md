# Async patterns

This library implements popular patterns for safe asynchronous programming in
C++. It does not dictate the only way to write asynchronous code, but following
the patterns will integrate with other asynchronous types such as FIDL bindings
and outgoing directories nicely:

- Each class is thread-unsafe and used from one async dispatcher.
- Divide classes along concurrency boundaries. Use message sending and task
  posting to synchronize between objects in different concurrency domains.
- Each asynchronous wait or task is owned by its receiver and silently canceled
  when the receiver is destroyed.

Code that follow these patterns can minimize locks and have simple destruction
logic, and still enables highly concurrent applications given many concurrency
domains. They may interface with other asynchronous programming styles, such as
those using locks and multi-threaded objects, via message sending.
