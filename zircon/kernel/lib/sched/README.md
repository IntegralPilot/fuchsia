# libsched

libsched (/libˈskej/) encapsulates the main utilities of kernel scheduling.
Despite its primary intended application within the kernel, this library aims to
remain environment-agnostic, especially for exercise within non-kernel, testing
contexts.

# Bandwidth model

_Bandwidth_ refers to the amount of execution time available to a thread over
time, and the _bandwidth model_ refers to the manner in which that time is
modeled and allocated to threads.

A thread's work is framed as happening within fixed-width, recurring time
intervals called _activation periods_. The duration is a property of the thread
and is referred to as its _period_. When the current activation period
_expires_, it is _reactivated_ and the time allocated to the thread within it is
reset. This chunking of work into periods arises naturally in many practical
contexts like the duration between VSyncs in graphical contexts and the
frequency of polling by device drivers. Accordingly, a period is better regarded
as a looser, slack-encoding, often externally-prescribed duration in which a
thread is expected to perform its work rather than a precise measure of the
duration of that intended work, which is instead referred to as the thread's
_capacity_.

A thread's work has two components: _firm_ and _flexible_. Firm work is that
which is _required_ to occur within a period; it is specified directly as the
thread property of _firm capacity_. Flexible work is that which happens on a
best-effort basis, utilizing remaining bandwidth - if any - after firm work has
been accounted for; it is specified indirectly as the thread property of
_flexible weight_. Flexible weight gives a conventional measure of the
importance of the desired flexible work, with a thread's _flexible capacity_
being derived from the proportion of its weight to the sum of all such weights
across the set of threads (naturally scaled by a firm utilization factor). The
_(total/effective) capacity_ of a thread is the sum of its firm and flexible
capacities, giving the total duration of execution time a thread expects within
an activation period.

All _runnable_ threads (i.e., the currently running thread or those queued and
ready to be run) contribute both firm and flexible demand. Further, threads that
become blocked will contribute _firm_ demand for the remainder of the activation
period in which that took place. This allows threads that quickly unblock to
complete the still-accounted-for firm work they set out to do in that period and
not be penalized.

## Thread selection

At all times, the thread that is selected next is the active one with the
earliest finish time (if any). In the event of a tie of finish times, the one
with the earlier start time is picked - and then in the event of a tie of start
times, the thread with the lowest address is expediently picked (which is a
small bias that should not persist across runs of the system).

This process is facilitated by `sched::RunQueue`.

## Preemption time

When a thread is selected, the _preemption time_ - the time at which the next
selection process should take place, potentially preempting the currently
executing thread - is calculated as well. This should be triggered as soon as
there is a state transition affecting the bandwidth of the just-selected thread
or the one that would become eligible next, and is calculated as the earlier of
the following times:

* When the currently selected thread finishes its activation period;
* When the currently selected thread would reach its capacity if allowed to
  continue uninterrupted;
* The starting time of a thread - if any - that would be eligible at the earlier
  of the above times and finish before the currently selected.
* The earliest finish time among all threads still blocked and within their
  current activation periods (allowing us to free up the firm demand they were
  holding onto).
