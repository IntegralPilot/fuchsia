# Reading configuration values

Drivers can use component framework's [structured configuration][structured-configuration]
to read values provided at product assembly time (see [providing configuration][providing-configuration]).
See the example [C++][example-driver-cpp] and [Rust][example-driver-rust] drivers.

## Include the configuration use in CML

In your driver CML, `use` the `config` capability with the name of your configuration:

```json5
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/cpp/meta/config_driver.cml" region_tag="use_config" adjust_indentation="auto" %}
```

## Set up the build

In the build file, you need to declare the manifest target manually, so that you can use the
build target to generate the library to access the configuration:

* {C++}

    ```gn
    {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/cpp/BUILD.gn" region_tag="manifest_declaration" adjust_indentation="auto" %}
    ```

* {Rust}

    ```gn
    {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/rust/BUILD.gn" region_tag="manifest_declaration" adjust_indentation="auto" %}
    ```

## Access the configuration in code

You can now access the configuration values in the code by using the generated library.

1.  Include the library dependency:

    * {C++}

        ```cpp
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/cpp/config_driver.cc" region_tag="include" adjust_indentation="auto" %}
        ```

    * {Rust}

        ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/rust/src/lib.rs" region_tag="include" adjust_indentation="auto" %}
        ```

1.  Use the library to access the configuration from the start arguments of the driver:

    * {C++}

        ```cpp
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/cpp/config_driver.cc" region_tag="use" adjust_indentation="auto" %}
        ```

    * {Rust}

        ```rust
        {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/config/rust/src/lib.rs" region_tag="use" adjust_indentation="auto" %}
        ```

<!-- Reference links -->

[example-driver-cpp]: /examples/drivers/config/cpp/
[example-driver-rust]: /examples/drivers/config/rust/
[providing-configuration]: /docs/development/drivers/driver_guides/configuration/providing_configuration.md
[structured-configuration]: /docs/reference/components/structured_config.md