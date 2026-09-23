keywords: docType:Guide,docType:ApiReference,category:FuchsiaDevelopment,category:FuchsiaTestAndDebug,category:FuchsiaTools,category:FuchsiaSDK,category:FuchsiaFidl
keywords_public: FIDL, key-value store, example, tutorial, client, server, interface definition, test harness, Fuchsia
description: This page provides a baseline example of a write-only key-value store in Fuchsia, covering FIDL interface definitions, client and server implementations, and a test harness.
<!-- These keywords are for search widget on fuchsia.dev. Do not remove. -->

# Key-value store: Baseline example

This page details how to create an example write-only key-value
store — defining interface definitions and a test harness as well as client/server implementations.

## Getting started {#baseline}

<<_baseline_tutorial.md>>

## What's next?

[Key-value store: Improving the design](/docs/development/languages/fidl/examples/key_value_store/improving-key-value-store.md) details how to add to this baseline key-value
store. Specifically, this details how to complete the following:

+   Adding support for reading from the store
+   Using generic values
+   Supporting nested key-value stores
+   Adding support for iterating the store
+   Enabling exporting backups