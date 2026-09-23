# Building and running a session {#building-and-running-a-session}

A session is the first product-specific component started on boot. The session
component is responsible for building a product's user experience.

## Booting into a session {#booting-into-a-session}

To boot into a session, do the following:

1. For a session to run at boot you need to configure the product build with
   the session's URL. Identify the component URL for your session:

      <pre><code>
      fuchsia-pkg://fuchsia.com/<var>pkg-name</var>#meta/<var>your_session.cm</var>
      </code></pre>

   Replace the following:

   - <var>pkg-name</var>: the package name
   - <var>your_session.cm</var>: the name of your session, including the `.cm`
     extension.

   For more information, see
   [`fuchsia-pkg`](/docs/reference/components/url.md#fuchsia-pkg) and
   [Package name](/docs/concepts/packages/package_url.md#package-name).

1. Update your `//local/BUILD.gn` to include `product_assembly_overrides`.

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
        url = "fuchsia-pkg://fuchsia.com/<var>pkg-name</var>#meta/<var>your_session.cm"
      }
    }
  }
  </code></pre>

1. Run the following command to include `your_session` in your base image and set
   the configuration values you defined in `//local:custom_session`.

   <pre class="prettyprint"><code class="devsite-terminal">
   fx set <var>product</var>.<var>board</var> --assembly-override=//local:<var>custom_session</var>
   </code></pre>

   `fx list-products` and `fx list-boards` will show lists of the products and
   boards available to be used in the `fx set` command. For more information on
   `fx` commands see the [fx documentation](/docs/development/build/fx.md).

1. Rebuild and re-pave the device.

   ```posix-terminal
   fx build
   fx ota
   ```

   This causes `session_manager` to start and launch your session.

For a full explanation of building a session component, see [Writing a Hello
World Session](/docs/development/sessions/writing-a-hello-world-session.md).

## Launch a session from the command line {#launch-a-session-from-the-command-line}

There are cases when you don't want your session to launch at boot but still
want to be able to launch it from the command line. `session_manager` needs
to be running to launch a session. The `session_manager` target
ensures `session_manager` itself starts, but does not autolaunch a session.

To launch a session from the command line, do the following:

1. Update your `//local/BUILD.gn` with `product_assembly_overrides` using `autolaunch`.

  <pre><code>
  import("//build/assembly/developer_overrides.gni")

  assembly_developer_overrides("custom_session") {
    base_packages = [
      "<var>//path/to/your/session</var>"
    ]
    platform = {
      session = {
        enabled = true
        autolaunch = false
      }
    }
    product = {
      session = {
        url = "fuchsia-pkg://fuchsia.com/<var>pkg-name</var>#meta/<var>your_session.cm"
      }
    }
  }
  </code></pre>

1. Add the `session_manager` target in the base dependency set, in
   addition to the session target.

   <pre class="prettyprint"><code class="devsite-terminal">
   fx set <var>product</var>.<var>board</var> --assembly-override=<var>//local:custom_session</var>
   </code></pre>

   `fx list-products` and `fx list-boards` will show lists of the products and
   boards available to be used in the `fx set` command. For more information on
   `fx` commands see the [fx documentation](/docs/development/build/fx.md).

1. Run the following command to rebuild and repave the device:

   ```posix-terminal
   fx build
   fx ota
   ```

   This causes `session_manager` to start without launching your session.

1. Your session can now be launched from the command line.

   Run the following command to launch your session:

   <pre class="prettyprint"><code class="devsite-terminal">ffx session launch fuchsia-pkg://fuchsia.com/<var>pkg-name</var>#meta/<var>your_session.cm</var></code></pre>

   For more information about the `ffx session` command, run
   `ffx session --help`. For more information about `ffx`, see the
   [`ffx documentation`](/docs/development/tools/ffx/overview.md).
