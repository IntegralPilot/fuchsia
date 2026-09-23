# Fake Boot Arguments for Security Package Delivery Tests

This component provides a fake `fuchsia.boot.Item` implementation for
security package delivery tests. For example, `fshost` depends on this
protocol to determine it's configuration.

The component manifest is declared as an incomplete shard so that configuration
details can be determined by the underlying test.
