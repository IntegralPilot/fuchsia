# FIDL bindings in Python

This page provides examples of different FIDL types and how they are
instantiated and used in Python.

Most of the examples focus on how types are translated from FIDL to Python.
If you would like to learn about implementng FIDL servers or handling
async code, check out the [tutorial][scripting-remote-actions] or the
[async Python best practices][best-practices] page.

## Bits and Enum types

Both bits and enum types are represented the same in Python, so this example
works for both.

* {FIDL}

  ```fidl {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlc/testdata/bits.test.fidl" region_tag="bindings_example" %}
  ```

* {Python}

  ```py {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/bindings.py" region_tag="bits" %}
  ```

This Python code above prints the following output:

```sh {:.devsite-disable-click-to-copy}
<MyBits.MY_FIRST_BIT: 1>
<MyBits.MY_OTHER_BIT: 2>
<MyBits.MY_FIRST_BIT|MASK|128: 133>
```

## Struct types

In Python, size limits are not enforced during construction, so if you were to
make a struct with an overly long string, for example, the example below would
not create an error for the user until attempting to encode the struct to bytes.
This goes for other composited structures as well, such as tables and unions.

* {FIDL}

  ```fidl {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlc/testdata/struct.test.fidl" region_tag="bindings_example" %}
  ```

* {Python}

  ```py {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/bindings.py" region_tag="struct" %}
  ```

This Python code above prints the following output:

```sh {:.devsite-disable-click-to-copy}
Simple(f1=5, f2=True)
```

If you were to attempt to construct this type without one of the fields, it
would raise a TypeError exception listing which arguments were missing.

## Table types

These are identical to struct types but without the strict constructor
requirements. Any fields not supplied to the constructor will be set to `None`.

* {FIDL}

  ```fidl {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlc/testdata/table.test.fidl" region_tag="bindings_example" %}
  ```

* {Python}

  ```py {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/bindings.py" region_tag="table" %}
  ```

This Python code prints the following output:

```sh {:.devsite-disable-click-to-copy}
EmptyTable()
SimpleTable(x=6, y=None)
SimpleTable(x=None, y=7)
SimpleTable(x=None, y=None)
```

## Union types

These work similar to struct types but only one variant can be specified.
A flexible union can support no variant being specified.

* {FIDL}

  ```fidl {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlc/testdata/union.test.fidl" region_tag="bindings_example" %}
  ```

* {Python}

  ```py {:.devsite-disable-click-to-copy}
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/bindings.py" region_tag="union" %}
  ```

This Python code prints the following output:

```sh {:.devsite-disable-click-to-copy}
<'PizzaOrPasta' object(pizza=Pizza(toppings=['pepperoni', 'jalapeños']))>
<'PizzaOrPasta' object(pasta=Pasta(sauce='pesto'))>
```

<!-- Reference links -->

[best-practices]: /docs/development/tools/fuchsia-controller/async-python.md
[scripting-remote-actions]: /docs/development/tools/fuchsia-controller/scripting-remote-actions.md
