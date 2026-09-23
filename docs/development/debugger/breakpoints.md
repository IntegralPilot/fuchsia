# Use breakpoints

Breakpoints stop execution when code is executed. To create a breakpoint, use
the `break` command and give it a location to break.

For example, to create a breakpoint on the `main` function:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
break main
Breakpoint 3 (Software) on Global, Enabled, stop=All, @ main
   180
 ◉ 181 int main(int argc, char**argv) {
   182     fbl::unique_fd dirfd;
```

There are several ways to express a breakpoint in zxdb. For example:

  * {Function name}

    You can specific a function name which matches functions with the name in
    any namespace:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    break main
    ```

  * {Member function}

    You can specify a member function or functions inside a specific namespace
    or class:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    break {{"<var>my_namespace</var>"}}::{{"<var>MyClass</var>"}}::{{"<var>MyFunction</var>"}}
    [zxdb] break ::{{"<var>OtherFunction</var>"}}
    ```

  * {Source and line}

    You can also specify a source file and the line number to break on:

    Note: Make sure to separate the source file name and line number with a colon.

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    break mymain.cc:22
    ```

  * {Line number}

    You can specify a line number within the current frame’s current source
    file. This is useful when you are stepping through code:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    break 23
    ```

  * {Memory address}

    You can specify a memory address:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    break 0xf72419a01
    ```

  * {Expression}

    You can specify an expression, see [Evaluate expressions][zxdb-expressions]
    for more information on expressions in zxdb. Prefixing with `*` treats the
    input that follows as an expression that evaluates to a specific address.
    This is useful when you work with hardware breakpoints.

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    break --type=write *&foo
    ```

## List breakpoints

To view all of the breakpoints, use `breakpoint`:

Note: This is the `breakpoint` noun. You can also use `bp` to express
`breakpoint`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
breakpoint
  # scope  stop enabled type     #addrs hit-count location
▶ 3 global all  false   software      1         0 machine.h:7
```

## Remove a breakpoint

To remove a specific breakpoint, give that breakpoint index as the context for
the `breakpoint <index> rm`.

For example, to clear `breakpoint 3`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
breakpoint 3 rm
Removed Breakpoint 3 enabled=false @ machine.h:7
```

Key Point: **GDB users:** `delete <index>` is mapped to `breakpoint <index> rm`.

## Clear breakpoints

To remove all breakpoints at a particular location, you do not need to specify
an index:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
clear
```

When you create or stop on a breakpoint, that breakpoint becomes the default
automatically. Whenever you run `clear` without a specific index, the command
clears the latest breakpoint that you hit.

`clear` can also take an optional location just like a `break` command. In this
way, it will try to clear all breakpoints at that location and ignore the
default breakpoint context.

Key Point: **GDB users:** `clear <number>` behaves the same in GDB and zxdb.

## Disable a breakpoint

For example, to disable breakpoint `3`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
breakpoint 3 disable
Disabled Breakpoint 3 enabled=false @ machine.h:7
   35   static constexpr SizeType InitialStackPointer(SizeType base, SizeType size) {
   36     // Stacks grow down on most machines.
 ◯ 37     return (base + size) & -kStackAlignment<SizeType>;
   38   }
   39 };
```

Note: If you do not provide a breakpoint index, the last breakpoint that you hit
is used.

To disable the current breakpoint:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
disable
Disabled Breakpoint 2 enabled=false @ main.rs:5
   24
   25 enum Services {
 ◯ 26     ComponentRunner(frunner::ComponentRunnerRequestStream),
   27     StarnixManager(fstarnixrunner::ManagerRequestStream),
   28     AttributionProvider(fattribution::ProviderRequestStream),
```

## Enable a breakpoint

After you have disabled a breakpoint, you may want to re-enable the
disabled breakpoint.

For example, to enable breakpoint `3`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
breakpoint 3 enable
Enabled Breakpoint 3 @ machine.h:7
   35   static constexpr SizeType InitialStackPointer(SizeType base, SizeType size) {
   36     // Stacks grow down on most machines.
 ◉ 37     return (base + size) & -kStackAlignment<SizeType>;
   38   }
   39 };
```

## Set and get breakpoint properties

You can also modify breakpoint properties with the `get` and `set` commands.

For example, to retrieve the `location` property from breakpoint `4`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
breakpoint 4 get location
location (locations)

  The location (symbol, line number, address, or expression) where this
  breakpoint will be set. See "help break" for documentation on how to specify.

location = machine.h:7
```

For example, to set the `location` property from breakpoint `4` to `machine.h:8`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
breakpoint 4 set location = machine.h:8
Set breakpoint 4 location = machine.h:8
```

## Conditional breakpoints

You can also configure a breakpoint to have conditionals. A conditional is an
expression that evaluates to either `true` or `false`. When you set a
conditional, the breakpoint does not trigger a stop unless the this conditional
is `true`.

For example, if you are debugging the `cobalt.cm` component:

For example, to add a conditional breakpoint location of `main.cc:352`:

  ```none {:.devsite-disable-click-to-copy}
  [zxdb] b main.cc:352 if command_line.has_argv0 == false
  Created Breakpoint 1 condition="command_line.has_argv0 == false" @ ../../src/myapp/bin/app/main.cc:352
    351   }
  ◉ 352   inspector.Health().Ok();
    353   loop.Run();
    354   FX_LOGS(INFO) << "Cobalt will now shut down.";
  ```

## Hardware data breakpoints

In zxdb, hardware breakpoints are exposed as a type of breakpoint rather than as
a separate **watchpoint**.

The processor can be set to break execution when it reads or writes certain
addresses. This can be useful to track down memory corruption.

You can create a hardware breakpoint when you use any of the following values
for the `type` property of a `break` command.

* `execute`
* `write`
* `read-write`

For example, to set a breakpoint of type `execute`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
break --type=execute myfile.rs:123
```

A `watch` is the same as using `break --type=read-write`. See
[`watch` command](#watch-command).

### `watch` command {#watch-command}

As a shortcut, the `watch` command takes the contents of a variable or the
result of an expression and set a data write breakpoint over its range:

Note: CPUs only support a limited number of hardware watchpoints, typically
around 4.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
watch i
[zxdb] watch foo[5]->bar
```

If you `watch` a variable on the stack and nobody touches it, you will often
see it hit in another part of the program when the stack memory is re-used.
If you get a surprising breakpoint hit, check that execution is still in the
frame you expect.

Key Point: **GDB Users**: `watch` evaluates the expression once and then sets a
breakpoint on the result. It won't re-evaluate the expression. In the above
example, it triggers when `bar` changes but not if `foo[5]` changes to point
to a different `bar`.

## Programmatic breakpoints

In some cases you may want to catch a specific condition in your code. To do this
you can insert a hard coded breakpoint in your code.

Note: This does not work in GCC Zircon builds.

Clang has a built-in:

```cpp
__builtin_debugtrap();
```

If zxdb is already attached to the process, it will stop as if a normal
breakpoint was hit. You can then `step` or `continue` from there. If the
debugger is not already attached, this will cause a crash.

[zxdb-expressions]: /docs/development/debugger/exceptions.md
