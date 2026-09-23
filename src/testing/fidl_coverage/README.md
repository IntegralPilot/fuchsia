# fidl_coverage: assistant to the CTF FIDL coverage system

## Dependencies

The fidl_coverage tool requires Python 3.

It's intended to be invoked by a CQ recipe for the CTF builder, where
it will scan test artifacts to see which FIDL methods were used, and
compare them to an API file that lists the methods we want to cover.

Its output is JSON-format to stdout, which will produce a useful artifact
for dashboarding coverage.
