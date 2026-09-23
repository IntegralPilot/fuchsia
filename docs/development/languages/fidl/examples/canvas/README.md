keywords: docType:Overview,category:FuchsiaTools,category:FuchsiaSDK,category:FuchsiaPowerAndPerformance,category:FuchsiaFidl
keywords_public: FIDL, example, Canvas, 2D line-drawing, data flow patterns, flow control, performance, client requests, draw operations, Fuchsia
description: This document provides an overview of the FIDL Canvas example, which demonstrates various data flow patterns and performance optimizations for a 2D line-drawing application in Fuchsia.
<!-- These keywords are for search widget on fuchsia.dev. Do not remove. -->

# FIDL example: Canvas

In this example, we start by creating a 2D line-drawing canvas, then proceed to
augment its functionality with various data flow patterns commonly used in FIDL,
such as implementing flow control on both sides of the connection, and improving
performance by reducing the number of message round trips.

## Getting started {#baseline}

<<_baseline_tutorial.md>>

## Improving the design {#variants}

Each of the following sections explores one potential way that we could iterate
on the original design. Rather than building on one another sequentially, each
presents an independent way in which the base case presented above may be
modified or improved.

<!-- DO_NOT_REMOVE_COMMENT (Why? See: /tools/fidl/scripts/canonical_example/README.md) -->

### Basic metering on client requests {#add_line_metered}

<<_add_line_metered_tutorial.md>>

### Clients explicitly request draw operations {#client_requested_draw}

<<_client_requested_draw_tutorial.md>>

<!-- /DO_NOT_REMOVE_COMMENT (Why? See: /tools/fidl/scripts/canonical_example/README.md) -->
