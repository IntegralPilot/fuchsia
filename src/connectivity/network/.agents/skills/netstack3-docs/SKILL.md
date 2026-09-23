---
name: netstack3-docs
description: Guide and pointers to key Netstack3 architecture and design documentation (CORE_BINDINGS, TENETS, IP_TYPES). Use this when making architectural changes or design decisions in Netstack3.
---

# Netstack3 Architecture & Design Principles

Before making significant architectural changes or design decisions in Netstack3, you MUST read the design documentation in `src/connectivity/network/netstack3/docs/`.

Specifically, refer to:
*   [CORE_BINDINGS.md](../../../netstack3/docs/CORE_BINDINGS.md)
    for details on the Core/Bindings split and input validation strategy.
*   [TENETS_AND_DESIGN_DECISIONS.md](../../../netstack3/docs/TENETS_AND_DESIGN_DECISIONS.md)
    for key tenets including API boundaries with traits (`XxxContext`), state
    sharing patterns (`with_state`), core public API design, and lock
    ordering guidelines.
*   [IP_TYPES.md](../../../netstack3/docs/IP_TYPES.md)
    and [STATIC_TYPING.md](../../../netstack3/docs/STATIC_TYPING.md)
    for guidelines on IP agnosticism and using the type system to guarantee
    invariants.
