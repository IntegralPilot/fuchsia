# Control thread execution

When you use zxdb, you can control the execution of threads to help you debug.
As you debug you can control execution through the use of the following:

* Thread

  A thread is a unit of execution within a process. It represents a single
  sequence of instructions that can be executed independently.

  To control execution through threads in zxdb, see [Threads](#threads).

* Stack frame

  A stack frame is a section of the call stack that is allocated when a function
  is called. It stores information needed for the function's execution, such as:

  * Local variables: Variables declared within the function.
  * Parameters: Values passed to the function.
  * Return address: The location in the code to return to after the
    function completes.

  To control execution through stack frames in zxdb, see
  [Stack Frames](#stack-frames).

## Threads

In zxdb, a `thread` is a [noun][zxdb-noun] that you can use with zxdb
[verbs][zxdb-verb].

To list the threads in the current process:

Note: This is the `thread` noun. You can also use `t` to express `thread`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
thread
  # State   Koid Name
▶ 1 Blocked 1323 initial-thread
  2 Running 3462 worker-thread
```

In some cases, you may notice that a thread is marked as `Blocked` which means
that the thread is stopped on a system call. Typically, when you are debugging
an asynchronous application this may also indicate a wait time.

Thread control commands only work on a suspended thread, not blocked or running
threads. There are several ways to suspend a thread:

* A [breakpoint][zxdb-breakpoints] stopped the thread.
* Use the [`pause`](#pause-command) command.

### `pause` a thread {#pause-command}

For example, to suspend thread `2` with the `pause` command:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
thread 2 pause
🛑 syscalls-x86-64.S:67
   65 m_syscall zx_port_create 60 2 1
   66 m_syscall zx_port_queue 61 2 1
 ▶ 67 m_syscall zx_port_wait 62 3 0
   68 m_syscall zx_port_cancel 63 3 1
   69 m_syscall zx_timer_create 64 3 1
```

When a thread is paused zxdb shows the current source code location. If a thread
is in a system call, like the example above, the source code location resolves
to the location in the assembly-language macro file that generated the system
call.

If you run `pause` without any additional context, zxdb pauses all threads of
all processes that are currently attached.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
pause
   508                 const zx_port_packet_t* packet))
   509
 ▶ 510 BLOCKING_SYSCALL(port_wait, zx_status_t, /* no attributes */, 3, (handle, deadline, packet),
   511                  (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, zx_time_t deadline,
   512                   zx_port_packet_t* packet))
🛑 $elf(SYSCALL_zx_port_wait) + 0x7 • syscalls.inc:510
```

### `continue` a thread {#continue-thread}

After you have paused a thread and started debugging an issue, you may want to
`continue` the thread. Continuing means resuming execution until your program
completes normally.

For example, to `continue` thread `1`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
thread 1 continue
```

If you run `continue` without any additional context, zxdb continues all the
threads of all attached processes.

For example:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
continue
```

### Stepping a thread

When a thread is paused you can control its execution. You can use any of these
commands:

Note: For more information on pausing a thread, see
[`pause` a thread](#pause-command).

* `finish` (`fi`)

  Exits the function and stops right after the call.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  finish
  ```

* `next` (`n`)

  Advances to the next line, stepping over function calls.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  next
  ```

* `nexti`

  Advances to the next instruction, but steps over call instructions for the
  target architecture.

  Note: In this context, a call instruction is `call` on x64 and `bl` on arm64.
  This does not work for all cases. For example, a manually set up call frame
  and a `jump` may result stepping into a new stack frame.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  nexti
  ```

* `ss`

  List function calls on the current line and step in to the call selected. This
  automatically completes any of the other calls that happen to occur first.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  ss
    1 std::string::string
    2 MyClass::MyClass
    3 HelperFunctionCall
    4 MyClass::~MyClass
    5 std::string::~string
    quit
  >
  ```

* `step` (`s`)

  Advances to the next code line. If a function call happens before the next
  line, that function is stepped into and execution stops at the
  beginning of that function.

  You can also supply an argument substring to match a specific function call.
  Function names that do not contain the argument substring  are skipped and
  only matching functions are stepped into.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  step
  [zxdb] step MyFunction
  ```

* `stepi`

  Advances exactly one machine instruction.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  stepi
  ```


* `until` (`u`)

  Given a line location, continues the thread until execution gets there. For
  example, to run until line `45` of the current file:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  until 45
  ```

  You can also run until execution gets back to a given stack frame:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  frame 2 until
  ```

## Stack frames {#stack-frames}

A stack frame is a function call. When a function calls another function, a new
frame is created. Listing the frames of a thread returns the call stack.

Note: You can only see the stack frames when a thread is suspended. See
[`pause` a specific thread](#pause-command).

To list the stack frames in the current thread:

Note: This is the `frame` noun. You can also use `f` to express `frame`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
frame
▶ 0 fxl::CommandLineFromIterators<const char *const *>() • command_line.h:203
  1 fxl::CommandLineFromArgcArgv() • command_line.h:224
  2 main() • main.cc:174
```

When you work with stack frames, `0` indicates the top of the stack, which
indicates the end of the execution. The bottom of the stack, which is the
highest stack frame number, indicates the start of the execution.

### Navigating stack frames {#navigate-stack-frames}

You can use the `up` and `down` commands to navigate the frame list.

For example, use `up` to navigate from the current frame `0` to frame `1`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
up
  1 fxl::CommandLineFromIterators<const char *const *>() • command_line.h:204
```

For example, use `down` to navigate from the current frame `1` to frame `0`:

```
[zxdb] down
  0 fxl::CommandLineFromIteratorsFindFirstPositionalArg<const char *const *>() • command_line.h:185
```

You can also navigate to a specific frame by using the `frame` command with a
frame number:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
frame 1
```

### Use `backtrace` for additional details {#backtrace}

In some cases, you may want to see additional address information that
stack frames don't provide. The `backtrace` command is identical to `frame` but
gives you more detailed address information as well as function parameters.

Note: This is the `backtrace` verb. You can also use `bt` to express
`backtrace`.

To list the stack frames in the current thread, but with more detailed
information, use `backtrace`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
backtrace
▶ 0 fxl::CommandLineFromIteratorsFindFirstPositionalArg<const char *const *>() • command_line.h:185
      IP = 0x10f982cf2ad0, BP = 0x66b45a01af50, SP = 0x66b45a01af38
      first = (const char* const*) 0x59f4e1268dc0
      last = (const char* const*) 0x59f4e1268dc8
      first_positional_arg = (const char* const**) 0x0
  1 fxl::CommandLineFromIterators<const char *const *>() • command_line.h:204
      IP = 0x10f982cf2ac0, BP = 0x66b45a01af50, SP = 0x66b45a01af40
      first = <'first' is not available at this address. >
      last = <'last' is not available at this address. >
...
```

### Use `list` to look at source code {#list}

Each stack frame has a code location. Use the `list` command to look at the
source code.

You can list code around the instruction pointer of specific stack frames.

For example, to `list` the source code around the instruction pointer of stack
frame `3`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
frame 3 list
```

When you use `list` without context, zxdb lists the source code
around the instruction pointer of the current stack frame:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
list
   183 inline CommandLine CommandLineFromIteratorsFindFirstPositionalArg(
   184     InputIterator first, InputIterator last,
 ▶ 185     InputIterator* first_positional_arg) {
   186   if (first_positional_arg)
   187     *first_positional_arg = last;
```

#### Additional use cases for `list`

Additionally, you can use `list` to list specific things:

* {Functions}

  Use `list` to list functions:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list MyClass::MyFunc
  ```

* {Files}

  Use `list` to list specific files:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list --all myfile.cc:1
  ```

* {File with line numbers}

  Use `list` to list specific files with specific line numbers:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list foo.cc:43
  ```

[zxdb-noun]: /docs/development/debugger/commands.md#noun
[zxdb-verb]: /docs/development/debugger/commands.md#verbs
[zxdb-breakpoints]: /docs/development/debugger/breakpoints.md