# Python devshell tools

This directory contains `fx` subcommands written in Python.

These tools are automatically included in builds.

## Adding a tool

See [fxrev.dev/1080462](https://fxrev.dev/1080462) for an example
of following these instructions.

The following instructions assume you are creating a tool called
`my-new-fx-tool`, which you can replace with the real name of your
tool.

1. Create a new directory at
`//tools/devshell/python/my-new-fx-tool`. The rest of the
instructions are relative to that directory unless otherwise
specified.
1. Create an OWNERS file containing your email and any shared owners'.
1. Create source Python files in this directory.
1. Create a `tests` directory for your test code.
1. Create a BUILD.gn file with the following contents:
    ```gn
    # Copyright 2024 The Fuchsia Authors. All rights reserved.
    # Use of this source code is governed by a BSD-style license that can be
    # found in the LICENSE file.

    import("//build/python/host.gni")
    import("//build/python/python_host_test.gni")
    ```
1. Add a `python_binary` to your BUILD.gn file:
    ```gn
    python_binary("my-new-fx-tool") {
        main_source = "main.py"  # Reference main source here
        sources = [ "main.py" ]  # Sources here
        deps = []                # Deps here
    }
    ```
1. Add a target for your tests to BUILD.gn:
    ```gn
    if (is_host) {
        python_host_test("my-new-fx-tool-test") {
            main_source = "tests/test_my_new_fx_tool.py"  # Test path here
            sources = [
                "main.py",                                # Allow import main.
                "tests/test_my_new_fx_tool.py",
            ]
        }
    }
    ```
1. Add groups for inclusion elsewhere to BUILD.gn:
    ```gn
    install_python_tool("install") {
        name = "my-new-fx-tool"
        binary = ":my-new-fx-tool"
    }

    group("tests") {
        testonly = true
        deps = [ ":my-new-fx-tool-test($host_toolchain)" ]
    }
    ```
1. Add those groups to `//tools/devshell/python`:
    ```gn
    group("tests") {
        testonly = true
        deps = [
            # ...
            "my-new-fx-tool:tests",
        ]
    }

    group("install") {
        deps = [
            # ...
            "my-new-fx-tool:tests",
        ]
    }
    ```
1. Add devshell manifest at `//tools/devshell/my-new-fx-tool.fx`,
replacing values in `<>`:
    ```
    # Copyright 2024 The Fuchsia Authors. All rights reserved.
    # Use of this source code is governed by a BSD-style license that can be
    # found in the LICENSE file.

    #### CATEGORY=Other <replace with correct category>
    #### EXECUTABLE=${HOST_TOOLS_DIR}/my-new-fx-tool
    ### <Short description>
    ## 'fx my-new-fx-tool --help' for instructions.
    ```

You can now run your tool with `fx my-new-fx-tool`.

## Running tests

Add the correct labels and execute tests:

```bash
fx set minimal.x64 --with-test //tools/devshell:tests
fx test --host //tools/devshell
```

