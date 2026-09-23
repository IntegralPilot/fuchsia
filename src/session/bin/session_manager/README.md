# session_manager

Reviewed on: 2021-10-01

`session_manager` is the component that runs and manages [session components](glossary.session-component).

## Building

Add the `session_manager` component to builds by including `--with-base
//src/session/bin/session_manager` in the `fx set` invocation followed by
rebuilding and re-paving the device.

Product configurations built on Session Framework (such as `fx set
workstation_eng.x64`) include `//src/session/bin/session_manager` by default.

## Running

`session_manager` is launched in one of two ways: manually or automatically on
system boot.

In general, running manually is useful during development, while running on boot
is desirable for a production configuration.

### Running manually

Use the [`ffx session`](/docs/development/tools/ffx/getting-started.md) tool
to launch a specific session:

```
ffx session launch fuchsia-pkg://fuchsia.com/your_session#meta/your_session.cm
```

### Launching a session on boot

`session_manager` attempts to launch a session on boot based on the contents of
its `product.session.url` configuration parameter.

To boot into a session, include `session_manager` and the session component in
the base package set and assign the URL of the session component to the product
configuration, update your `//local/BUILD.gn` file to use the
`assembly_developer_overrides` template:

  <pre><code>
  import("//build/assembly/developer_overrides.gni")

  assembly_developer_overrides("<var>custom_session</var>") {
    base_packages = [
      "<var>//path/to/your/session</var>"
    ]
    platform = {
      session = {
        enabled = true
      }
    }
    product = {
      session = {
        url = "fuchsia-pkg://fuchsia.com/your_package#meta/your_session.cm"
      }
    }
  }
  </code></pre>

Then include the new build target in your `fx set`:

```
fx set minimal.x64 --assembly-override=//local:<var>custom_session</var>
```

Re-build and ota, and the device will boot into `session_manager` and
autolaunch your session.

### Temporarily disabling launch-on-boot

If your development device runs a product that specifies `product.session.url`,
you may nonetheless want to stop the session from launching (e.g., to avoid log
spam from the session, or make changes to the device's state _before_ the
session launches).

You can disable autolaunching by updating your `//local/BUILD.gn` accordingly:

  <pre><code>
  import("//build/assembly/developer_overrides.gni")

  assembly_developer_overrides("<var>custom_session</var>") {
     platform = {
        session = {
           enabled = true
           autolaunch = false
        }
     }
     product = {
        session = {
           url = "fuchsia-pkg://fuchsia.com/your_package#meta/your_session.cm"
        }
     }
  }
  </code></pre>

## Testing

Unit tests for `session_manager` are included in the `session_manager_tests`
package, and integration tests are included in the
`session_manager_integration_tests` package.

Both can be included in your build by adding `--with //src/session:tests` to
your `fx set`.

```
fx test session_manager_tests
fx test session_manager_integration_tests
```

## Source layout

The entry point for `session_manager` is in `src/main.rs` with implementation
details in other files within `src/`.

Unit tests are co-located with the code, while integration tests are in
[`//src/session/tests`](/src/session/tests).

[glossary.session-component]: /docs/glossary.md#session-component
