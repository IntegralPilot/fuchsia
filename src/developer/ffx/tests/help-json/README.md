# ffx help-json tests

These tests exercise getting the json encoded help from ffx.

This is done by running a collection of command lines of the form
`ffx --machine json [command] --help` and then check that the json returned
has the expected command structure.

There are golden tests that detect changes in the content, by contrast, these tests
are making sure the processing of the command line works properly.
