Verify that a product can reboot and with the expected reboot reason.

This is akin to //src/tests/reboot/ but with a different test framework that
allows it to run on more products and boards as well as assert on higher-level
concepts.

### Running the test

While the test is added to some bot configurations, it's unlikely to be in the
local configuration a developer is building locally so to run the test locally,
one has to add it to what they are building:

```
$ fx set ... --with-host //src/tests/end_to_end/reboot_reason:reboot_reason_test
$ fx test -o --e2e reboot_reason_test
```

Note that by construction, the test will reboot the device so be advised!
