# utc-handle-vendor

Serves writable UTC handles for tests.

Some tests require mutable UTC handles, which is not usually available on
Fuchsia, except under special circumstances.

The component serves an existing FIDL protocol `fuchsia.time/Maintenance`, which
is normally only served by Component Manager, and only served to Timekeeper.

See the component manifest (`meta/default.cml`) for provision details.

