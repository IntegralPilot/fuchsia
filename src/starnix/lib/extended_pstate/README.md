# extended_pstate

This provides helpers for saving, restoring, and resetting extended processor
state such as floating point control/status registers and floating point/vector
registers. It is intended to be used in conjunction with Zircon's restricted mode
feature which manages general purpose register state.
