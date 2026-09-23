# Python testing

Host-side Python tests in Fuchsia are executed via `fx test`, CI, and CQ using
the Fuchsia-vendored Python interpreter (`fuchsia-vendored-python`).

## Defining a Host Test

Use the [`python_host_test`](/build/python/python_host_test.gni) GN template
to define a test target.

Test `.py` files must use the standard shebang:

```shell
#!/usr/bin/env fuchsia-vendored-python
```

### Example `BUILD.gn`

```gn
import("//build/python/python_host_test.gni")

if (is_host) {
  python_host_test("my_script_test") {
    main_source = "my_script_test.py"
    sources = [ "test_helper.py" ]
    libraries = [ "//path/to/library:lib" ]
  }
}

group("tests") {
  testonly = true
  public_deps = [ ":my_script_test($host_toolchain)" ]
}
```

## Key GN Parameters

* `main_source`: (Required) Path to the primary `.py` test entry point.
* `libraries`: (Optional) List of `python_library` labels imported by the test.
* `enable_mypy`: (Optional) Enables MyPy static type checking. Defaults to `true`.

## Running Tests

Run tests locally using `fx test`:

```posix-terminal
fx test //path/to/my_test:my_script_test
```

