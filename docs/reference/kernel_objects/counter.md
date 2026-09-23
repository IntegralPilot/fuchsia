# Counter

## NAME

counter - A semaphore-like object for synchronizing across processes

## SYNOPSIS

A counter is like an [event], but with an integer that can be incremented,
decremented, read, or written.

## DESCRIPTION

Counter is a synchronization tool allowing processes to coordinate and
synchronize their actions.  A counter contains a signed 64-bit integer and is
somewhat similar to a counting semaphore.

Counter is designed to be used with [`zx_object_wait_one()`],
[`zx_object_wait_many()`], or [`zx_object_wait_async()`].

## SIGNALS

In addition to the standard user signals (**ZX_USER_SIGNAL_0** through
**ZX_USER_SIGNAL_7**), counter has two signals that are automatically
asserted/deasserted based on the value:

**ZX_COUNTER_NON_POSITIVE**  indicates the value is less than or equal to zero.

**ZX_COUNTER_POSITIVE**  indicates the value is greater than or equal to zero.

## SYSCALLS
 - [`zx_counter_create()`] - create a counter
 - [`zx_counter_add()`] - add to a counter
 - [`zx_counter_read()`] - read a counter's value
 - [`zx_counter_write()`] - write a counter's value


[event]: /reference/kernel_objects/event.md
[`zx_counter_create()`]: /reference/syscalls/counter_create.md
[`zx_counter_add()`]: /reference/syscalls/counter_add.md
[`zx_counter_read()`]: /reference/syscalls/counter_read.md
[`zx_counter_write()`]: /reference/syscalls/counter_write.md
[`zx_object_wait_one()`]: /reference/syscalls/object_wait_one.md
[`zx_object_wait_many()`]: /reference/syscalls/object_wait_many.md
[`zx_object_wait_async()`]: /reference/syscalls/object_wait_async.md
