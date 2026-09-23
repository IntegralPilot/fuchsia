# Get started with zxdb

Zxdb is an asynchronous debugger that allows the user to interact with the
debugger while processes or threads are running or stopped.

Zxdb uses a noun, verb, and command model for typed commands. This document
highlights the main functionality of the zxdb debugger.

## Debugger model

When working with zxdb, you can perform most actions by combining a `noun` and
a `verb`.

* [Noun](#noun)
* [Verbs](#verbs)

## Noun {#noun}

The possible nouns (and their abbreviations) are:

* `breakpoint` (`bp`)

  Select or list breakpoints.

* `filter`

  Select or list process filters.

* `frame` (`f`)

  Select or list stack frames.

* `global` (`gl`)

  Global override for commands.

* `process` (`pr`)

  Select or list process contexts.

* `sym-server`

  Select or list symbol servers.

* `thread` (`t`)

  Select or list threads.

### Listing nouns

If you type a noun by itself, it lists the available objects of that type. For
example:

  * {breakpoint}

    List all of the breakpoints in the session:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    breakpoint
    # scope  stop enabled type     Condition                       #addrs hit-count location
    1 global all  true    software command_line.has_argv0 == false      1         0 ../../src/cobalt/bin/app/cobalt_main.cc:352
    ```

  * {frame}

    List stack frames in the current thread:

    Note: You must `pause` the thread before you can list the stack frames.

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    frame
    ▶ 0 fxl::CommandLineFromIterators<const char *const *>() • command_line.h:203
      1 fxl::CommandLineFromArgcArgv() • command_line.h:224
      2 main() • main.cc:174
    ```

  * {process}

    List attached processes:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    process
      # State       Koid Name
    ▶ 1 Not running 3471 debug_agent_unit_tests.cm
    ```

  * {thread}

    List threads in the current process:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    thread
      # State   Koid Name
    ▶ 1 Blocked 1348 initial-thread
      2 Blocked 1356 some-other-thread
    ```


### Selecting active nouns {#selecting-active-nouns}

You can select a noun and its respective index to make it the active noun for
subsequent commands. When you set a new active noun, it returns information
about the new active noun. For example:

  * {thread}

    Select thread 3 to be the active noun for future commands:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    thread 3
    Thread 3 Blocked koid=9940 worker-thread
    ```

  * {breakpoint}

    Select breakpoint 2 to be the active noun:

    ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
    breakpoint 2
    Breakpoint 2 (Software) on Global, Enabled, stop=All, @ MyFunction
    ```

## Verbs

In zxdb, verbs are used in conjunction with nouns, which specify a zxdb object,
to perform debugging actions. For a full list of zxdb verbs, see
[`verbs.h`][zxdb-verbs-code].

By default, a verb  such as `run`, `next`, `print`, etc... applies to the
current active nouns (for more information on active nouns, see
[Selecting active nouns](#selecting-active-nouns)).

### Data display verbs

  * Memory display verbs are covered in [memory](memory.md).
  * Register display verbs are covered in [assembly](assembly.md).

## Help

The zxdb debugger has a built-in help system:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
help
```

You can also get help on a specific command. For example, to see the help of the
`step` command:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
help step
```

## Attributes and settings

zxdb debugger objects have settings associated with them. You can specify some
of these settings to personalize zxdb.

### Retrieve

You can use the `get` verb to list the settings for a given object.

For example, you can get the attributes of the active `process`:

Note: The `get` command can be used on all zxdb nouns. When using `get`, you can
also specify a specific process such as `process 1 get`.

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
process get
  debug-stepping false
  display        <empty>
  show-stdout    true
  source-map     • /b/s/w/ir/x/w/fuchsia-third_party-rust=/usr/local/home/user/fuchsia/out/default/host_x64/../../../prebuilt/third_party/rust/linux-x64/lib/rustlib/src/rust
  vector-format  double
```

You can also use the `get` verb with a specific attribute to list the attribute
and help associated with it.

For example, to get the help of the `debug-stepping` attribute:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
process get debug-stepping
debug-stepping (bool)

  Enable very verbose debug logging for thread stepping.

  This is used by developers working on the debugger's internal thread
  controllers.

debug-stepping = false
```

### Set

You can use the `set` verb to set the settings or attributes of a given object.

For example, you can set the `show-stdout` attribute of the active `process` to
`true`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
process set show-stdout true
Set process 1 show-stdout = true
```

Some settings are hierarchical. A thread inherits settings from its process,
which in turn inherits settings from the global scope. If you use the `get` verb
without context or parameters, it lists the global settings and the specific
settings for the current process and thread.

You can set a global setting to apply to all threads and processes without
specific overrides, or override a specific context.

For example, you can set the `show-stdout` global setting to `false`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set show-stdout false       # Applies to all processes with no override.
Set global show-stdout = false
```

Some settings are stored as lists. For example, you can use any of these
examples to set your `symbol-paths`:

Note: Elements in a list are space-separated. You can use double quotes
to specify a path that contains a space.

* Use `=` to specify a new value:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set symbol-paths = /tmp/symbols /fuchsia-settings/symbols "/fuchsia settings/symbols"
  Set global symbol-paths =
    • /tmp/symbols
    • /fuchsia-settings/symbols
    • "/fuchsia settings/symbols"
  ```

* Use `+=` to append to the existing values:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  set symbol-paths += /tmp/symbols2
  Set global symbol-paths =
    • /tmp/symbols
    • /fuchsia-settings/symbols
    • "/fuchsia settings/symbols"
    • /tmp/symbols2
  ```

### Languages

Note: C++ and Rust are supported.

zxdb evaluates expressions based on the programming language from the current
stack frame. If the current frame's language is different, zxdb defaults to C++.

By default zxdb uses a language of `auto`. You can overwrite the default
language with the `set` verb.

Note: You should rarely need to set the language.

For example, to set the default language to `rust`:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
set language rust
Set global language = rust
```

[zxdb-verbs-code]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/debug/zxdb/console/verbs.h;l=36
