# Best practices on using async Python in Fuchsia Controller

This page discusses the common task flows of async Python as well as possible
pitfalls when using async Python.

Before you continue, check out this
[official documentation][python-async-docs]{:.external} on `asyncio` and
the [tutorial][scripting-remote-actions] for Fuchsia Controller.

## Background

The following items concern asynchronous code in general, not necessarily specific
to Python:

* Async code is not necessarily multi-threaded. In the case of
  Fuchsia Controller related code, almost everything is single-threaded.
* Async code runs inside of a loop (also known as an executor), which is
  yielded to whenever an await happens.

Because of these, it can make certain things difficult to debug, like
if you have a background task that intends to run forever but raises an
exception. If you never await this task, the exception is only visible in
logs.

## Common pitfalls

### Not holding references to tasks

If you launch a task in a loop, for example:

```py {:.devsite-disable-click-to-copy}
# Don't do this!
asyncio.get_running_loop().create_task(foo_bar())
```

and you do not hold a reference to the value returned, it is possible that the
Python garbage collector may cancel and delete this task at an arbitrary point
in the future.

Always make sure to store the tasks you create in a variable, for example:

```py {:.devsite-disable-click-to-copy}
# Always store a variable!
foo_bar_task = asyncio.get_running_loop().create_task(foo_bar())
```

Then ensure that the lifetime of the task is tied to what needs it. If this is
a specific test case, store the task as a member of the test case class,
canceling it during teardown.

### Mixing blocking code with async code

While testing your code, it's common for some experiments to throw in a `sleep`
statement for certain events. Make sure not to mix up `asyncio.sleep` with
`time.sleep`. The former returns a coroutine while the latter blocks the whole
loop.

For example:

```py {:.devsite-disable-click-to-copy}
import asyncio
import time


async def printer():
    """ Prints a thing."""
    print("HELLO")


async def main():
    task = asyncio.get_running_loop().create_task(printer())
    {{ "<strong>" }} time.sleep(1) # THIS WILL BLOCK EXECUTION. {{ "</strong>" }}
    task.cancel()

if __name__ == "__main__":
    asyncio.run(main())
```

The code above would print out nothing. However, it would print `HELLO` if you
replaced the `time.sleep(1)` with `await asyncio.sleep(1)`, which yields to the
async loop.

### FIDL server errors

Given the nature of async programming, it's possible that you can have a runtime
error in your FIDL server implementation and not know it, especially if your
FIDL server implementation is connected to a component on a Fuchsia device.

This, unfortunately, is difficult to debug and may only surface as a
`PEER_CLOSED` error on the device due to your FIDL server implementation crashing.

One approach (for debugging) is to add a callback to the task that checks if
an exception has occurred in the task and sends something to logs (or even hard
crash the program).

For example:

```py {:.devsite-disable-click-to-copy}
def log_exception(task):
    try:
        _ = task.result()
    except Exception as e:
        # Can log the exception here for debugging.

task = asyncio.get_running_loop().create_task(foobar())
task.add_done_callback(log_exception)
```

## Common task flows

### Synchronizing tasks

To start, it's recommended to take a look at the
[Python doc page][python-synchronization-docs]{:external} for a primer on
synchronization objects available. Note that these objects are _not_ thread-safe.
Some of them merit examples, however.

#### Waiting for multiple tasks

You'll likely need to wait for multiple tasks to complete. This can be done
using [`asyncio.wait`][asyncio-wait]{:.external}. If you want to do something
similar to the `select` syscall, set the `return_when` parameter to
`asyncio.FIRST_COMPLETED`, for example:

```py {:.devsite-disable-click-to-copy}
done, pending = await asyncio.wait(
    [task1, task2, task3],
    return_when=asyncio.FIRST_COMPLETED
)
```

The result contains a tuple of tasks (in this case, `done` and `pending`). Which
can be treated like any other task object. So for the `done` object, one can
iterate over these and collect the results by checking the `result()` function.

#### Running async code inside synchronous code

At the time of writing, most of Fuchsia Controller code is being called from
synchronous code. To ensure tasks can run in the background, it may make sense
to keep an instance of a loop via `fuchsia_async_extension.get_loop()`.

The reason for this is that `asyncio` tasks must remain on a single loop. This
is also the case for synchronization objects like `asyncio.Queue`.

If the code you interact with is sufficiently simple (running a single FIDL
method at a time), you can do this using:

```py {:.devsite-disable-click-to-copy}
fuchsia_async_extension.get_loop().run_until_complete(...)
```

But if you need to run tasks or handle synchronization, you'll want to
encapsulate things in a class containing a loop. Just make sure that if this is
in a test or an object, there is teardown code for shutting down the loop,
for example:

```py {:.devsite-disable-click-to-copy}
class FidlHandlingClass():

    def __init__(self):
        self._loop = fuchsia_async_extension.get_loop()
        # Can launch tasks here.
        self._important_task = self._loop.create_task(foo())

    def __del__(self):
        self._important_task.close()
        self._loop.close()
```

You can also extend the class with a decorator so that the loop is implicitly
used in all public functions (though they will appear synchronous to callers)
for example:

```py {:.devsite-disable-click-to-copy}
from functools import wraps

class FidlInstance():

    def __init__(self, proxy_channel):
        self._loop = fuchsia_async_extension.get_loop()
        self.echo_proxy = fidl_fuchsia_developer_ffx.EchoClient(proxy_channel)

    def __del__(self):
        self._loop.close()

    def _asyncmethod(f):
        @wraps(f)
        def wrapped(inst, *args, **kwargs):
            coro = f(inst, *args, **kwargs)
            inst._loop.run_until_complete(coro)
        return wrapped

    @_asyncmethod
    async def echo(self, string: str):
        return await self.echo_proxy.echo_string(value=string)
```

Then the class above can be invoked using the following code:

```py {:.devsite-disable-click-to-copy}
client_channel = ...
inst = FidlInstance(client_channel)
print(inst.echo("foobar"))
```

While this instance can run inside a non-async context, it is still runs
async code. However, this can make it much easier to read and write overall.

### Adapting code to support async Python

FIDL is a language designed around being async. However, test frameworks like
`Mobly` expect synchronous Python functions for running tests. If you have an
interface that you'd like to test and it is primarily used in synchronous
Python, then this applies to you.

`fuchsia-controller` comes with some mixin code under its `wrappers` module to
support using async code in synchronous contexts, in particular `AsyncAdapter`,
which can be used as a wrapper or a mixin, and the decorator `asyncmethod`.

When used in conjunction with each other, this creates a setup where all code
run in a given instance will execute against a common `asyncio` event loop.

For most cases async Python can be run in synchronous Python with the use
of `asyncio.run()`. However, when you may have queues or async tasks expected to
check on state across function calls, this becomes prohibitively difficult to
do. Each invocation of `asyncio.run()` creates a totally new event loop, which
can create headaches for managing asynchronous data structures, which expect to
be used in just one event loop at a time. By keeping a single `asyncio` event
loop, you can avoid running into these exceptions.

For example, let's say you're writing a `Mobly` test case, but the class you're
exposing relies on some underlying async Python, you can write your code like
so to allow for its use in `Mobly`:

```py {:.devsite-disable-click-to-copy}
import asyncio
from mobly import asserts, base_test, test_runner
from fuchsia_controller_py.wrappers import AsyncAdapter, asyncmethod

# AsyncAdapter should be included first in the list of base classes.
class ClassYouWantToTest(AsyncAdapter, SomeBaseClass):

    def __init__(self):
        super().__init__()
        self.async_init()

    @asyncmethod
    async def async_init(self):
        self.queue: asyncio.Queue[int] = asyncio.Queue()
        await self.queue.put(1)

    @asyncmethod
    async def function_we_care_about(self) -> int:
        got = await self.queue.get()
        self.queue.task_done()
        return got


class ExampleTest(base_test.BaseTestClass):

    def test_case_example(self) -> None:
        """Example... doesn't really do much useful."""
        c = ClassYouWantToTest()
        asserts.assert_equal(c.function_we_care_about(), 1)

if __name__ == "__main__":
    test_runner.main()
```

The method `function_we_care_about` will be wrapped so that functionally the
following code is being executed when the `Mobly` test case is executed:

```py {:.devsite-disable-click-to-copy}
def function_we_care_about(self) -> None:
  coro = self._function_we_care_about_impl()
  self._mixin_asyncio_loop.run_until_complete(coro)
```

Note: If using `AsyncAdapter` as a mixin, it should be the first thing included
in the inheritance, particularly if using `Mobly`, as this will ensure that the
loop logic is initialized properly.

<!-- Reference links -->

[asyncio-wait]: https://docs.python.org/3/library/asyncio-task.html#asyncio.wait
[scripting-remote-actions]: /docs/development/tools/fuchsia-controller/scripting-remote-actions.md
[python-async-docs]: https://docs.python.org/3/library/asyncio.html
[python-synchronization-docs]: https://docs.python.org/3/library/asyncio-sync.html
