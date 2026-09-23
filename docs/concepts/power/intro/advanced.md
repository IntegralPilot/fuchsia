# Advanced Usage

Now we'll cover a couple of advanced use cases of power framework. These are
particularly geared towards optimizing for performance. It may be useful to read
[Power framework in depth][in_depth] before looking at these advanced use
cases.

Both of the advanced usages focus on reducing the number of power framework
interactions and are good (possibly the only) options for high event rate
systems. Most of the time when the system is awake it is because there is a
high-level policy keeping it that way, for example, only sleep if we've gone 5
minutes without a keyboard or mouse event. This means that _most_ of the time
the system will stay awake long enough for us to get our work done and it is
only when the system is waking from sleep or trying to suspend that we need to
interact with power framework. In other words, it is only at system state
transition edges into and out of suspension where power framework interaction is
required. At these edges our code must do additional work.

[in_depth]: in_depth_overview.md
