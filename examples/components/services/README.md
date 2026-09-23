# Services example

This directory contains an example of routing
[service capabilities](/docs/concepts/components/capabilities/service.md)
in [Component Framework](/docs/concepts/components/introduction.md)
and aggregating multiple service instances from a component collection.

## Building

If these components are not present in your build, they can be added by
appending `--with //examples` to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples --with //examples:tests
$ fx build
```

## Running

This example is built to run inside the Test Runner Framework. Run the example
using the following command:

-   **C++**

    ```bash
    $ ffx test run fuchsia-pkg://fuchsia.com/service-examples-cpp#meta/default.cm
    ```

-   **Rust**

    ```bash
    $ ffx test run fuchsia-pkg://fuchsia.com/service-examples-rust#meta/default.cm
    ```

When the above command is run, you can see the following output in the test console
in addition to the test cases passing:

```
[bank_branch] INFO: creating BankAccount provider url=provider-a#meta/default.cm name=a
[bank_branch] INFO: open exposed dir of BankAccount provider name=a url=provider-a#meta/default.cm
[bank_branch] INFO: creating BankAccount provider name=b url=provider-b#meta/default.cm
[bank_branch] INFO: open exposed dir of BankAccount provider name=b url=provider-b#meta/default.cm
[account_providers:a] INFO: starting bank account provider balance=23 name=A
[account_providers:b] INFO: starting bank account provider balance=42 name=B
[bank_branch] INFO: retrieved account owner=A balance=23
[bank_branch] INFO: debiting account owner=A
[account_providers:a] INFO: balance updated account=A balance=18
[bank_branch] INFO: retrieved account owner=B balance=42
[bank_branch] INFO: debiting account owner=B
[account_providers:b] INFO: balance updated balance=37 account=B
```
