keywords: docType:Overview,docType:Guide,docType:ApiReference,category:FuchsiaDevelopment,category:FuchsiaFidl
keywords_public: FIDL, calculator, example, protocol, tutorial, Fuchsia, development
description: This document provides an overview of a basic FIDL calculator example, serving as a starting point for learning how to construct and iterate on FIDL protocols in Fuchsia.
<!-- These keywords are for search widget on fuchsia.dev. Do not remove. -->

# FIDL example: Calculator

This Calculator example shows how to construct a FIDL protocol with
bare-minimum functionality. You will build on this example by modifying the
methods to potentially return errors, composing a FIDL protocol with another,
and showing usage of other primitives.

## Getting started {#baseline}

<<_baseline_tutorial.md>>

## Improving the design {#variants}

Each of the following sections explores one potential way that you could iterate
on the original design. Rather than building on one another sequentially, each
presents an independent way in which the base case presented above may be
modified or improved.

<!-- DO_NOT_REMOVE_COMMENT (Why? See: /tools/fidl/scripts/canonical_example/README.md) -->

<!-- /DO_NOT_REMOVE_COMMENT (Why? See: /tools/fidl/scripts/canonical_example/README.md) -->
