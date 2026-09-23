// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/standalone-test/standalone.h>
#include <lib/zx/clock.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/port.h>
#include <lib/zx/resource.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/port.h>
#include <zircon/syscalls/system.h>
#include <zircon/threads.h>

#include <chrono>

#include <zxtest/zxtest.h>

#include "../needs-next.h"

NEEDS_NEXT_SYSCALL(zx_system_suspend_enter);

// TODO(https://fxbug.dev/440105800): Remove these helpers and go back to using
// `std::atomic_ref` once this bug has been fixed.
extern "C" void wake_report_fetch_add(zx_futex_t* val, zx_futex_t amt);
extern "C" zx_futex_t wake_report_load(zx_futex_t* val);

namespace {

using namespace std::literals::chrono_literals;
using BootTimePair = std::pair<zx::time_boot, zx::time_boot>;

// The only user-mode accessible wake sources defined in the system today are
// interrupt objects. Interrupt objects come in two main flavors: those which
// have been bound to a port object and deliver their signals via port packets,
// and those which have a thread which blocks on the interrupt object directly
// via `zx_interrupt_wait`.
//
// The decision of which to use is up to the user, however in the kernel, the
// code which handles the behavior of each is rather different.  The way that
// interrupts deliver their signals, and their trigger timestamps, and how the
// become acknowledged is all different, and both approaches need to be tested.
//
// So, we define two test "flavors": "PortBound" and "ThreadBound".  Then, we
// template our tests on the test flavor and run every test for both version.
// We expect the same behavior for each test, but always exercise both versions.
enum class TestFlavor {
  PortBound,
  ThreadBound,
};

// A "wake source" object in the following tests represents a virtual interrupt
// object which we are going to signal, acknowledge, and verify behaviors of.
// The way in which we do this will depend on the flavor of wake source we are
// using (PortBound or ThreadBound).  While the two flavors of wake source have
// different ways to perform basic tasks (such as acknowledging the interrupt),
// the also share a lot of identical state which must be tracked for various
// validation tasks.
//
// WakeSourceBase is the common base class for all the wake sources used by
// these tests.  The specific flavors of wake sources will all derive from this
// base class.
class WakeSourceBase {
 public:
  WakeSourceBase() = default;
  ~WakeSourceBase() = default;

  void Init() {
    constexpr uint32_t options = ZX_INTERRUPT_VIRTUAL | ZX_INTERRUPT_WAKE_VECTOR;
    ASSERT_OK(zx::interrupt::create(*standalone::GetIrqResource(), 0, options, &handle_));

    zx_info_handle_basic_t info;
    ASSERT_OK(handle_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr));
    koid_ = info.koid;
    snprintf(expected_name_, sizeof(expected_name_), "VirtIRQ %ld", koid_);
  }

  void Reset() {
    initial_signal_time_ = std::nullopt;
    last_signal_time_ = std::nullopt;
    last_ack_times_ = std::nullopt;
    signal_count_ = 0;
    pending_acks_ = 0;
    has_been_reported_ = false;
    was_ever_acked_ = false;
  }

  zx::interrupt handle_;
  zx_koid_t koid_{ZX_KOID_INVALID};
  char expected_name_[ZX_MAX_NAME_LEN]{0};

  std::optional<zx::time_boot> initial_signal_time_;
  std::optional<zx::time_boot> last_signal_time_;
  std::optional<BootTimePair> last_ack_times_;
  uint32_t signal_count_{0};
  uint32_t pending_acks_{0};
  bool has_been_reported_{false};
  bool was_ever_acked_{false};
};

// SpecializeWakeSource is the specific version of a wake source templated on
// the TestFlavor.  Each version is fully-specialized, and this is the base
// version.
template <TestFlavor>
class SpecializedWakeSource;

// The specialized definition of a port-bound wake source.
template <>
class SpecializedWakeSource<TestFlavor::PortBound> : public WakeSourceBase {
 public:
  SpecializedWakeSource() = default;
  ~SpecializedWakeSource() { Shutdown(); }

  static zx_status_t SetUp() { return zx::port::create(ZX_PORT_BIND_TO_INTERRUPT, &irq_port_); }

  static zx_status_t TearDown() {
    irq_port_.reset();
    return ZX_OK;
  }

  // Initialization of a PortBound wake source involves simply binding the
  // interrupt object to the port shared by all PortBound Interrupts.
  void Init() {
    ASSERT_NO_FAILURES(WakeSourceBase::Init());
    EXPECT_OK(handle_.bind(irq_port_, koid_, 0));
  }

  void Shutdown() { handle_.reset(); }

  // Validating that a PortBound interrupt object has delivered its signal
  // involves:
  //
  // 1) Waiting for a port packet to show up.
  // 2) Verifying that the key of the packet matches the key we expect (we use
  //    the KOID of the interrupt object as the key for async wait operations).
  // 3) Verifying that this is actually an interrupt packet.
  // 4) Verifying that the trigger time reported was the trigger time we used
  //    when triggering the virtual interrupt in the first place.
  void WaitForAndValidateSignal(zx::time_boot expected_trigger_time) {
    zx_port_packet_t pkt;
    ASSERT_OK(irq_port_.wait(zx::time::infinite_past(), &pkt));
    EXPECT_EQ(koid_, pkt.key);
    EXPECT_EQ(ZX_PKT_TYPE_INTERRUPT, pkt.type);
    EXPECT_EQ(expected_trigger_time.get(), pkt.interrupt.timestamp);
  }

  // Validating that there are no signals pending just means verifying that
  // there are no port packets waiting to be read.
  void VerifyNoSignal() {
    zx_port_packet_t pkt;
    EXPECT_EQ(ZX_ERR_TIMED_OUT, irq_port_.wait(zx::time::infinite_past(), &pkt));
  }

  // Ack'ing PortBound interrupts is simple.  Just call the ack method on the
  // interrupt handle.
  void Ack() { EXPECT_OK(handle_.ack()); }

 private:
  static inline zx::port irq_port_;
};

// The specialized definition of a thread-bound wake source.
template <>
class SpecializedWakeSource<TestFlavor::ThreadBound> : public WakeSourceBase {
 public:
  SpecializedWakeSource() = default;
  ~SpecializedWakeSource() { Shutdown(); }

  static zx_status_t SetUp() { return ZX_OK; }
  static zx_status_t TearDown() { return ZX_OK; }

  // Performing the basic validate/acknowledge operations for a ThreadBound wake
  // source is a bit more complicated than it is for a PortBound wake source.
  // That said, it starts with each ThreadBound wake source having its own
  // thread, and having precise control of when that thread blocks in a call to
  // `zx_interrupt_wait()`.
  //
  // See the implementation of `Thread()` for more details.
  void Init() {
    ASSERT_NO_FAILURES(WakeSourceBase::Init());
    ZX_ASSERT(!thread_);
    thread_ = std::make_unique<std::thread>([this] { return Thread(); });
  }

  // Shutdown involves a specific sequence of signaling which should cause the
  // dedicated work thread to exit, then joining and cleaning up the thread.
  void Shutdown() {
    if ((thread_ != nullptr) && !shutdown_now_) {
      // Set the shutdown flag first.
      shutdown_now_ = true;

      // Close the underlying interrupt handle.  This will kick the thread out
      // of any interrupt wait operation it might happen to be in.
      handle_.reset();

      // Bump the ack_trigger_ and attempt to wake_all on the ack futex.  This
      // should kick us out of any "wait for ack" state we may be in.
      TriggerAck();
      zx_futex_wake(&ack_trigger_, 0xFFFFFFFF);

      // Wait for the thread to exit, then free it.
      thread_->join();
      thread_.reset();
    }
  }

  // To verify that an interrupt object has become signaled, we need to wait
  // until our thread is blocking (or trying to block) on our futex, and is
  // ready to be told to ack the interrupt.  Then, we need to verify that the
  // timestamp delivered to user-mode via `zx_interrupt_wait` matches what we
  // expect.
  void WaitForAndValidateSignal(zx::time_boot expected_time) {
    ASSERT_FALSE(shutdown_now_);
    WaitForReadyToAck();
    EXPECT_EQ(expected_time.get(), last_trigger_time_.get());
  }

  // Verifying that a ThreadBound wake source has no pending signals is a bit
  // more tricky that what we need to do for PortBound interrupts.  Zircon
  // interrupt objects in the general case do not participate (much) in the
  // standard signaling system.  For an interrupt object which is an actual HW
  // interrupt, there is no way to simply check that the interrupt is not
  // signaled.
  //
  // Thankfully, we are using virtual interrupts, so we can take advantage of
  // one of the zircon features which is specific to virtual interrupt objects.
  // Specifically, we can example the `ZX_VIRTUAL_INTERRUPT_UNTRIGGERED` which
  // (as the name implies) is set when the interrupt is not signaled and
  // waiting for acknowledgement.
  void VerifyNoSignal() {
    ASSERT_FALSE(shutdown_now_);
    zx_signals_t observed;
    handle_.wait_one(ZX_VIRTUAL_INTERRUPT_UNTRIGGERED, zx::time_monotonic::infinite_past(),
                     &observed);
    EXPECT_TRUE(observed & ZX_VIRTUAL_INTERRUPT_UNTRIGGERED);
  }

  // Ack'ing a ThreadBound wake source involves allowing our thread to block on
  // the wait source again.  See the comment in `Thread()` for how this whole
  // state machine works.
  void Ack() {
    // Start by making sure that the thread has made it to the point where it
    // has started to attempt to block on the futex.  Record the current value
    // for the "interrupt wait count" when we know that the thread is waiting
    // for the command to ack the interrupt.  This will be important for the
    // second phase of the Ack operation.
    ASSERT_FALSE(shutdown_now_);
    WaitForReadyToAck();
    const uint32_t pre_ack_interrupt_wait_count = interrupt_wait_count_;

    // Wake the thread from the futex.  This involves changing the ack trigger
    // value, so that a thread which is in the process of blocking will end up
    // not blocking after all, then attempt to wake the thread in case it has
    // made it all of the way down to the blocked state.
    TriggerAck();
    zx_futex_wake(&ack_trigger_, 0xFFFFFFFF);

    // Now wait until we are sure that the thread has actually acked the
    // interrupt.  This is when the ack time will be recorded in the wake
    // report, and we need to make sure that has happened before we can generate
    // a wake report and verify that the recorded ack time is reasonable.
    //
    // So, we need to make certain that the thread has called zx_interrupt_wait
    // at least once.  What complicates things, however, is that the thread may
    // Not end up blocking in the interrupt.  If there is a pending interrupt,
    // the thread will simply bounce off of the wait and immediately move back
    // to the WaitForAck state, so we cannot use a simple static state (such as
    // WaitingForInterrupt) to tell if an attempt was made.  Instead, we use a
    // counter, similar to the ack_trigger_ counter.  The thread will increment
    // this counter right before each attempt, and we knew what the counter
    // value was when it was blocked in the ack futex, so all we have to do is
    // wait for it to change and we know that an attempt was made.
    //
    // After that, we only need to wait until the thread is blocked, either in
    // the interrupt or in the futex.  After that, we can be certain that the
    // ack timestamp has been recorded for the wake source in the kernel.
    while (pre_ack_interrupt_wait_count == interrupt_wait_count_) {
      RelaxThread();
    }
    WaitForThreadBlocked();
  }

 private:
  enum class State {
    WaitForTrigger,
    WaitForAck,
  };

  // In a few different places during thread-bound tests, we need to spin waiting
  // for a thread to achieve a particular state in the kernel (in order for the
  // tests to be deterministic).  We _could_ just spin on the thread, but  it is a
  // bit more polite to just sleep for a small amount of time, which is how we
  // define our "relax" operation.
  void RelaxThread() { std::this_thread::sleep_for(200us); }

  void TriggerAck() {
    // TODO(https://fxbug.dev/440105800): Go back to using `std::atomic_ref`
    // instead of the intrinsic once this bug has been fixed.
#if 0
    std::atomic_ref(ack_trigger_).fetch_add(1u);
#else
    wake_report_fetch_add(&ack_trigger_, 1u);
#endif
  }

  void WaitForReadyToAck() {
    ASSERT_FALSE(shutdown_now_);
    while (ready_to_ack_ == false) {
      RelaxThread();
    }
  }

  void WaitForThreadBlocked() {
    while (true) {
      const uint32_t current_state = GetThreadState();
      if ((current_state == ZX_THREAD_STATE_BLOCKED_INTERRUPT) ||
          (current_state == ZX_THREAD_STATE_BLOCKED_FUTEX)) {
        break;
      }
    }
    RelaxThread();
  }

  uint32_t GetThreadState() {
    zx_info_thread_t info;
    zx::unowned_thread thread_handle{native_thread_get_zx_handle(thread_->native_handle())};
    const zx_status_t status =
        thread_handle->get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr);
    ZX_ASSERT(status == ZX_OK);
    return info.state;
  }

  // The main work thread state machine.  There are two main phases for the
  // thread's operation.  In the first phase, it blocks on the interrupt object
  // and waits for it to become triggered.  In the second phase, it waits in a
  // futex for the test control thread to tell it that it should try to block
  // again, which implicitly ack's the interrupt.
  //
  // The test control thread manages this thread's behavior via a few different
  // atomic variables (which use CST memory ordering just to keep things simple;
  // performance is not the most important thing here).
  //
  // + `ready_to_ack_`: This bool tells test control thread that the work thread
  //   has unblocked from the interrupt wait operation, has recorded the last
  //   trigger time, and is attempting to block in the futex.
  // + `interrupt_wait_count_`: This records the number of times that the work
  //   thread has attempted to make a call to `zx_interrupt_wait`.  See `Ack()`
  //   for more details, but a simple boolean (like `ready_to_ack_`) is not
  //   enough here.  After the control thread tells the work thread to ack the
  //   interrupt, it is possible for the thread to attempt to block only to
  //   discover that there is already another pending interrupt signaled in the
  //   interrupt object.  If we managed this with a single state variable
  //   (ReadyToAck, WaitingForInterrupt), the work thread could signal that it
  //   is waiting for an interrupt, then bounce off the wait syscall and
  //   immediately move to back to the ReadyToAck state, setting up a race where
  //   the control thread might never see that the work thread had made an
  //   attempt to block on the interrupt.  So, we use a counter instead.  The
  //   work thread increments this counter just before it attempts to block, and
  //   the control thread can observe the counter just before it releases the
  //   work thread from the futex block operation.  This allows the control
  //   thread to know that a new attempt was made, even if that immediately
  //   resulted in the work thread "unblocking" and moving on to waiting to be
  //   told to deliver another ack.
  // + `ack_trigger_`: Similar to the interrupt wait count, the ack trigger is
  //   used to break a potential race during futex blocking.  It's the classic
  //   futex race; the work thread tells the control thread that it is blocked
  //   on the futex (`ready_to_ack_ == true`), and then attempts to block.  The
  //   control thread sees that the thread "has blocked" and tries to wake it
  //   up, but there is no one waiting yet, so the wake operation fails to wake
  //   anyone, and the system locks up.  We avoid this race by having the work
  //   thread observe the `ack_trigger_` value before telling the control thread
  //   that it is about to block, then passing that value to the kernel's
  //   futex_wait operation as the expected futex value.  When the control
  //   thread want to wake a thread which should be blocked (or just about to
  //   block), it first bumps this counter.  The kernel will validate this value
  //   while holding all of the proper futex locks to prevent a concurrent wait
  //   operation.  If the validate check fails (control thread won the race),
  //   the work thread will just unwind instead of blocking.  Otherwise, it will
  //   move forward and block, dropping the internal futex locks as it goes and
  //   allowing a wake operation (but only after it has successfully blocked).
  //
  int Thread() {
    while (!shutdown_now_) {
      zx::time_boot trigger_time;
      interrupt_wait_count_ += 1;
      handle_.wait(&trigger_time);

      if (shutdown_now_) {
        break;
      }

      last_trigger_time_ = trigger_time;
      // TODO(https://fxbug.dev/440105800): Go back to using `std::atomic_ref`
      // instead of the intrinsic once this bug has been fixed.
#if 0
      const uint32_t expected_ack_trigger = std::atomic_ref(ack_trigger_).load();
#else
      const uint32_t expected_ack_trigger = wake_report_load(&ack_trigger_);
#endif

      ready_to_ack_ = true;
      zx_futex_wait(&ack_trigger_, expected_ack_trigger, ZX_HANDLE_INVALID, ZX_TIME_INFINITE);
      ready_to_ack_ = false;
    }
    return 0;
  }

  std::unique_ptr<std::thread> thread_;
  zx::time_boot last_trigger_time_{0};

  std::atomic<bool> shutdown_now_{false};
  std::atomic<bool> ready_to_ack_{false};
  std::atomic<uint32_t> interrupt_wait_count_{0};
  zx_futex_t ack_trigger_{0};
};

template <TestFlavor kTestFlavor>
class WakeReportTests : public zxtest::Test {
 public:
  using WakeSource = SpecializedWakeSource<kTestFlavor>;

  void SetUp() override {
    NEEDS_NEXT_SKIP(zx_system_suspend_enter);
    ASSERT_OK(WakeSource::SetUp());

    for (WakeSource& s : irq_wake_sources_) {
      ASSERT_NO_FAILURES(s.Init());
      ++active_irq_wake_source_count_;
    }

    const zx_status_t status =
        zx::resource::create(*standalone::GetSystemResource(), ZX_RSRC_KIND_SYSTEM,
                             ZX_RSRC_SYSTEM_CPU_BASE, 1, nullptr, 0, &system_cpu_resource_);
    ASSERT_OK(status);

    // Make certain we have discarded any pending wake report entries which may
    // be lingering from a previous test.
    ASSERT_EQ(ZX_OK,
              DoSuspend(zx::time_boot::infinite_past(),
                        ZX_SYSTEM_SUSPEND_OPTION_DISCARD | ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY));
    ResetLastSuspendOp();
  }

  void TearDown() override { ASSERT_OK(WakeSource::TearDown()); }

 protected:
  enum class ExpectAcked { No, Yes };

  static inline constexpr uint32_t kInterruptWakeSourceCount = 4;
  static inline constexpr uint32_t kWakeSourceCount = kInterruptWakeSourceCount + 1;
  static inline constexpr zx_koid_t kDeadlineWakeSourceKoid = 1;
  static inline constexpr const char kDeadlineWakeSourceName[ZX_MAX_NAME_LEN] = "suspend timeout";

  const zx::resource& system_cpu_resource() const { return system_cpu_resource_; }
  auto& irq_wake_sources() { return irq_wake_sources_; }

  zx_wake_source_report_header_t& report_hdr() { return report_hdr_; }
  auto& entry_buffer() { return entry_buffer_; }
  std::span<zx_wake_source_report_entry_t> entries() { return entries_; }

  void WaitForAndValidateSignal(size_t ndx, zx::time_boot expected_trigger_time) {
    ASSERT_LT(ndx, irq_wake_sources_.size());
    WakeSource& s = irq_wake_sources_[ndx];
    ASSERT_NO_FAILURES(s.WaitForAndValidateSignal(expected_trigger_time));
  }

  void VerifyNoSignal() {
    for (WakeSource& s : irq_wake_sources_) {
      s.VerifyNoSignal();
    }
  }

  zx::time_boot TriggerWakeSource(size_t ndx) {
    const zx::time_boot trigger_time = zx::clock::get_boot();

    // A small lambda used to work around the fact that we need to return a
    // trigger time, but a failed test ASSERT returns void.
    [&]() -> void {
      ASSERT_LT(ndx, irq_wake_sources_.size());
      WakeSource& s = irq_wake_sources_[ndx];

      // If this source has already shown up in a report, and we are triggering it
      // again, we need to reset the bookkeeping we use to track the expected
      // values for the next time a wake report entry for this source shows up in a
      // report.
      if (s.has_been_reported_) {
        s.Reset();
      }

      ASSERT_OK(s.handle_.trigger(0, trigger_time));

      // We've successfully triggered our interrupt, so increase the count of
      // acks we expect will be needed.  Note that interrupt objects can
      // effectively buffer, at most, 2 pending acks.  For example, consider a
      // port bound interrupt object.
      //
      // 1) The first time it is signaled, it immediately sends a port packet.
      // 2) The second time it is signaled, it records the timestamp of the
      //    signal time, but does not send another packet.  The packet from the
      //    first trigger operation needs to be read and explicitly acked first.
      // 3) The 3rd and subsequent times the object becomes signaled, no action
      //    will be taken.  We already have another packet ready to send, and
      //    when we do, that packet (and the ack it demands) represents the
      //    trigger of the second and all subsequent trigger operations.
      s.pending_acks_ = std::min(2u, s.pending_acks_ + 1);

      // Only read the pending port packet if our pending ack count is exactly
      // one.  If our pending ack count has become 2, it implies that we have
      // been signaled at least twice with no acks, and there is no second port
      // packet waiting to be read (yet);
      if (s.pending_acks_ == 1) {
        s.WaitForAndValidateSignal(trigger_time);
      }

      const uint32_t prev_count = s.signal_count_++;
      if (prev_count) {
        EXPECT_TRUE(s.initial_signal_time_.has_value());
      } else {
        s.initial_signal_time_ = trigger_time;
      }

      s.last_signal_time_ = trigger_time;
    }();

    return trigger_time;
  }

  void AckWakeSource(size_t ndx) {
    ASSERT_LT(ndx, irq_wake_sources_.size());
    WakeSource& s = irq_wake_sources_[ndx];

    if (s.pending_acks_ > 0) {
      const zx::time_boot before_ack_boot = zx::clock::get_boot();
      EXPECT_NO_FAILURES(s.Ack());
      s.last_ack_times_ = BootTimePair{before_ack_boot, zx::clock::get_boot()};
      s.was_ever_acked_ = true;

      // If we have more than one pending ack, then don't expect this wake
      // source to indicate that it "has been reported" the next time it becomes
      // reported.  When we ack an interrupt which has been signaled twice, it
      // effectively becomes ack'ed and then immediately becomes signaled again.
      // While the first signal has been reported, this second one has not been
      // just yet.
      if (s.pending_acks_ > 1) {
        s.has_been_reported_ = false;
      }
      --s.pending_acks_;
    } else {
      EXPECT_TRUE(false);
    }
  }

  void DestroyWakeSource(size_t ndx) {
    ASSERT_LT(ndx, irq_wake_sources_.size());
    ASSERT_GT(active_irq_wake_source_count_, 0);

    WakeSource& s = irq_wake_sources_[ndx];
    ASSERT_NE(ZX_HANDLE_INVALID, s.handle_.get());
    s.Shutdown();
    --active_irq_wake_source_count_;
  }

  zx_status_t DoSuspend(zx::time_boot deadline, uint32_t options = 0,
                        uint32_t entry_buffer_count = kWakeSourceCount) {
    ZX_DEBUG_ASSERT(entry_buffer_count <= entry_buffer_.size());
    uint32_t actual_entries = 0;

    // Zero out our out-parameter buffers so we are certain that anything in
    // those buffers after the call must have come from the kernel.
    report_hdr_ = zx_wake_source_report_header_t{0};
    for (zx_wake_source_report_entry_t& e : entry_buffer_) {
      e = zx_wake_source_report_entry_t{0};
    }

    before_last_request_ = zx::clock::get_boot();
    const zx_status_t result =
        zx_system_suspend_enter(system_cpu_resource_.get(), deadline.get(), options, &report_hdr_,
                                entry_buffer_count ? entry_buffer_.data() : nullptr,
                                entry_buffer_count, entry_buffer_count ? &actual_entries : nullptr);
    after_last_request_ = zx::clock::get_boot();

    if (result == ZX_OK) {
      // The time of the generated report has to exist in-between the
      // before/after times we latched around the syscall itself.
      EXPECT_LE(before_last_request_.get(), report_hdr_.report_time);
      EXPECT_GE(after_last_request_.get(), report_hdr_.report_time);

      // If we passed the "REPORT_ONLY" flag, then there should be no defined
      // "suspend start time" since we never attempted to suspend at all, just
      // fetch some more of the report.
      //
      // Otherwise, since the call succeeded, there should be a defined suspend
      // start time which exists between the last-request timestamps.
      if (options & ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY) {
        EXPECT_EQ(zx::time_boot::infinite().get(), report_hdr_.suspend_start_time);
      } else {
        EXPECT_LE(before_last_request_.get(), report_hdr_.suspend_start_time);
        EXPECT_GE(after_last_request_.get(), report_hdr_.suspend_start_time);
      }

      // The total number of wake sources in the system has to be the number of
      // wake sources we have explicitly created (but not yet destroyed),
      // plus one more for the internal deadline wake source.
      EXPECT_EQ(active_irq_wake_source_count_ + 1, report_hdr_.total_wake_sources);

      // Both the total number of entries which were returned in the entry
      // buffer, and the number which are still waiting to be reported, have to
      // be to be less than or equal to the total number of wake sources in the
      // system.
      //
      // Note: we cannot assume that the sum of reported and unreported is also
      // less than the total number of wake sources in the system.  This is
      // because, as the report is generated, a wake source (A) which has been
      // added to the report can become acknowledge and re-signaled, and be
      // waiting to be reported (again) at the end of report generation, where
      // the number of entries waiting to be reported is finally recorded.
      EXPECT_LE(actual_entries, report_hdr_.total_wake_sources);
      EXPECT_LE(report_hdr_.unreported_wake_report_entries, report_hdr_.total_wake_sources);

      entries_ = std::span{entry_buffer_.begin(), actual_entries};
    } else {
      ResetLastSuspendOp();
    }

    return result;
  }

  void VerifyTimeoutReported() {
    const zx_wake_source_report_entry_t* e = FindReportKoid(kDeadlineWakeSourceKoid);
    ASSERT_NOT_NULL(e);
    EXPECT_STREQ(kDeadlineWakeSourceName, e->name);
    EXPECT_LE(before_last_request_.get(), e->initial_signal_time);
    EXPECT_GE(after_last_request_.get(), e->initial_signal_time);
    EXPECT_EQ(e->initial_signal_time, e->last_signal_time);
    EXPECT_EQ(e->initial_signal_time, e->last_ack_time);
    EXPECT_EQ(0, e->flags);
    EXPECT_EQ(1u, e->signal_count);
  }

  void VerifyWakeSourceReported(size_t ndx, ExpectAcked expect_acked) {
    ASSERT_LT(ndx, irq_wake_sources_.size());
    WakeSource& s = irq_wake_sources_[ndx];

    const zx_wake_source_report_entry_t* e = FindReportKoid(s.koid_);
    ASSERT_NOT_NULL(e);

    // We've already checked that KOID matches (by finding-by-koid).  Now check
    // the name, count, and flags.
    EXPECT_STREQ(s.expected_name_, e->name);
    EXPECT_EQ(s.signal_count_, e->signal_count);

    const uint32_t expected_flags =
        ((expect_acked == ExpectAcked::No) ? ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_SIGNALED : 0u) |
        (s.has_been_reported_ ? ZX_SYSTEM_WAKE_REPORT_ENTRY_FLAG_PREVIOUSLY_REPORTED : 0u);
    EXPECT_EQ(expected_flags, e->flags);

    // Now check our timestamps, making sure that they are bounded by the
    // timestamps we took when we manually triggered and acknowledged the
    // source.
    ASSERT_TRUE(s.initial_signal_time_.has_value());
    EXPECT_EQ(s.initial_signal_time_.value().get(), e->initial_signal_time);

    ASSERT_TRUE(s.last_signal_time_.has_value());
    EXPECT_EQ(s.last_signal_time_.value().get(), e->last_signal_time);

    // If we were ever ack'ed, we should have a last_ack_time.
    if (s.was_ever_acked_) {
      ASSERT_TRUE(s.last_ack_times_.has_value());
      EXPECT_LE(std::get<0>(s.last_ack_times_.value()).get(), e->last_ack_time);
      EXPECT_GE(std::get<1>(s.last_ack_times_.value()).get(), e->last_ack_time);
    } else {
      EXPECT_FALSE(s.last_ack_times_.has_value());
      EXPECT_EQ(ZX_TIME_INFINITE, e->last_ack_time);
    }

    // Record that we have seen an entry for this wake-source at least once.  We
    // will reset our internal bookkeeping the next time we trigger the source.
    s.has_been_reported_ = true;
  }

  void VerifyNothingToReport() {
    EXPECT_OK(DoSuspend(zx::time_boot::infinite_past()));
    EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
    EXPECT_EQ(1u, entries().size());
    VerifyTimeoutReported();
  }

  const zx_wake_source_report_entry_t* FindReportKoid(zx_koid_t koid) const {
    for (const zx_wake_source_report_entry& e : entries_) {
      if (e.koid == koid) {
        return &e;
      }
    }
    return nullptr;
  }

  WakeSource* FindWakeSourceKoid(zx_koid_t koid) {
    for (WakeSource& s : irq_wake_sources_) {
      if (s.koid_ == koid) {
        return &s;
      }
    }
    return nullptr;
  }

  void ResetLastSuspendOp() {
    entries_ = std::span<zx_wake_source_report_entry_t>{};
    before_last_request_ = zx::time_boot::infinite_past();
    after_last_request_ = zx::time_boot::infinite_past();
  }

  void DoBadReportRequests();
  void DoSingleWakeSource();
  void DoMultiWakeSource();
  void DoAcked();
  void DoAckedStillPending();
  void DoDiscard();
  void DoDestructionNoAck();
  void DoDestructionYesAck();
  void DoAlreadyTimedOut();
  void DoReportOnly();
  void DoSmallReportBuffer();
  void DoMultipleSignals();

 private:
  zx::resource system_cpu_resource_;
  std::array<WakeSource, kInterruptWakeSourceCount> irq_wake_sources_;
  zx_wake_source_report_header_t report_hdr_;
  std::array<zx_wake_source_report_entry_t, kWakeSourceCount> entry_buffer_;
  std::span<zx_wake_source_report_entry_t> entries_;
  zx::time_boot before_last_request_{zx::time_boot::infinite_past()};
  zx::time_boot after_last_request_{zx::time_boot::infinite_past()};
  uint32_t active_irq_wake_source_count_{0};
};

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoBadReportRequests() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  constexpr zx::time_boot deadline = zx::time_boot::infinite_past();
  uint32_t actual_entries{0};

  struct TestVector {
    zx_wake_source_report_entry_t* evt_buffer;
    const uint32_t num_entries;
    uint32_t* actual_entries_ptr;
  };

  {
    // A user is requesting a report whenever the header pointer passed to
    // suspend_enter is valid. Any combination of entry buffer arguments but
    // {nullptr, 0, nullptr} is illegal. Try all of the combinations and verify
    // that they all fail.
    std::array kTestVectors = {
        TestVector{entry_buffer().data(), 0, nullptr},
        TestVector{nullptr, kWakeSourceCount, nullptr},
        TestVector{entry_buffer().data(), kWakeSourceCount, nullptr},
        TestVector{nullptr, 0, &actual_entries},
        TestVector{entry_buffer().data(), 0, &actual_entries},
        TestVector{nullptr, kWakeSourceCount, &actual_entries},
        TestVector{entry_buffer().data(), kWakeSourceCount, &actual_entries},
    };

    for (TestVector& v : kTestVectors) {
      EXPECT_EQ(ZX_ERR_INVALID_ARGS,
                zx_system_suspend_enter(system_cpu_resource().get(), deadline.get(), 0u, nullptr,
                                        v.evt_buffer, v.num_entries, v.actual_entries_ptr));
    }
  }

  {
    // When a report is requested, users may choose to fetch just the header, or
    // fetch the header and entries waiting to be reported.  What they cannot do is
    // pass a set of "inconsistent" entry buffer parameters.  IOW, if the pointer
    // to the entry buffer is nullptr, the num entries and actual entries arguments
    // must be {0, nullptr}, and if the pointer to the entry buffer is non-null,
    // the other arguments must be { >0, non-null }.  Try all of the illegal
    // combinations and make sure they all fail.
    std::array kTestVectors = {
        TestVector{entry_buffer().data(), 0, nullptr},
        TestVector{nullptr, kWakeSourceCount, nullptr},
        TestVector{entry_buffer().data(), kWakeSourceCount, nullptr},
        TestVector{nullptr, 0, &actual_entries},
        TestVector{entry_buffer().data(), 0, &actual_entries},
        TestVector{nullptr, kWakeSourceCount, &actual_entries},
    };
    for (TestVector& v : kTestVectors) {
      zx_wake_source_report_header_t* hdr = &report_hdr();
      EXPECT_EQ(ZX_ERR_INVALID_ARGS,
                zx_system_suspend_enter(system_cpu_resource().get(), deadline.get(), 0u, hdr,
                                        v.evt_buffer, v.num_entries, v.actual_entries_ptr));
    }
  }

  // The DISCARD and REPORT_ONLY flags are the only valid flags right now.  Make
  // sure that all other flags are rejected.
  constexpr uint64_t kValidFlags =
      ZX_SYSTEM_SUSPEND_OPTION_DISCARD | ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY;
  for (uint32_t i = 0; i < (sizeof(uint64_t) << 3); ++i) {
    const uint64_t bad_flag = uint64_t{1} << i;
    if ((bad_flag & kValidFlags) != 0) {
      continue;
    }

    uint32_t actual_entries;
    EXPECT_EQ(ZX_ERR_INVALID_ARGS,
              zx_system_suspend_enter(system_cpu_resource().get(), deadline.get(), bad_flag,
                                      &report_hdr(), entry_buffer().data(), kWakeSourceCount,
                                      &actual_entries));
  }

  // If a user wants to only generate a report, and not actually attempt to
  // enter suspend, they pass the REPORT_ONLY flag, but they *must* also pass a
  // buffer which to hold (at least) the report header.  Make sure that the
  // request is explicitly rejected if the flag is set, but no buffer is
  // supplied.
  EXPECT_EQ(
      ZX_ERR_INVALID_ARGS,
      zx_system_suspend_enter(system_cpu_resource().get(), deadline.get(),
                              ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY, nullptr, nullptr, 0, nullptr));
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoSingleWakeSource() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Signal one of our sources, and make sure that it both prevents us from
  // going into suspend, but is also reported as a wake report entry.
  TriggerWakeSource(0);

  // Attempt to go into suspend twice.  We should immediately bounce out both
  // times; the main difference is that on the second attempt, our wake report
  // entry will report itself as having been already reported.
  for (uint32_t i = 0; i < 2; ++i) {
    EXPECT_OK(DoSuspend(zx::time_boot::infinite()));

    // After our attempt to suspend, the report should indicate that exactly one
    // entry was reported, and that there are no more entries to report.
    EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
    EXPECT_EQ(1u, entries().size());

    // Verify that the source we triggered was the source that we reported, and
    // agrees with the state we expect as recorded by calls to
    // (Trigger|Ack)WakeSource.
    VerifyWakeSourceReported(0, ExpectAcked::No);
  }

  // Finally, ack the wake source, and make sure that there is nothing left to
  // report.
  AckWakeSource(0);
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoMultiWakeSource() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Signal more than one source, and make sure that they are all reported.
  for (uint32_t i = 0; i < irq_wake_sources().size(); ++i) {
    TriggerWakeSource(i);
  }

  // Attempt to enter suspend kWakeSourceCount times, acknowledging a single
  // source each time.  We expect to see all of our sources reported except for
  // the ones previously acked each time that we do.
  for (uint32_t i = 0; i < irq_wake_sources().size(); ++i) {
    const uint32_t expected_entries = static_cast<uint32_t>(irq_wake_sources().size()) - i;
    EXPECT_OK(DoSuspend(zx::time_boot::infinite()));
    EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
    EXPECT_EQ(expected_entries, entries().size());

    for (uint32_t j = i; j < irq_wake_sources().size(); ++j) {
      VerifyWakeSourceReported(j, ExpectAcked::No);
    }

    // Ack one of our sources and go around again.
    AckWakeSource(i);
  }

  // Now that all of our sources have been acked, there should be nothing left
  // to report.
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoAcked() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Make sure that wake-entries for wake sources which have already been
  // acknowledged are reported, but only once.
  constexpr uint32_t kSignalCount = 4;
  for (uint32_t i = 0; i < kSignalCount; ++i) {
    TriggerWakeSource(0);
    AckWakeSource(0);
  }

  // Request a report, and expect to see an entry reporting that source 0 was
  // triggered and acknowledged.  Note, because the source has been
  // acknowledged, there is nothing stopping the system from going into suspend.
  // To keep the test running, we need to pass a deadline in the past, and we
  // expect to see two entries in the report when we wake up: The IRQ source
  // (which didn't cause the wakeup) and the deadline source (which did).
  EXPECT_OK(DoSuspend(zx::time_boot::infinite_past()));
  EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
  EXPECT_EQ(2u, entries().size());
  VerifyWakeSourceReported(0, ExpectAcked::Yes);
  VerifyTimeoutReported();

  // Now request another report, there should be nothing in it.
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoAckedStillPending() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Signal and acknowledge one of our sources multiple times, but leave it in
  // the pending state and make sure that the  wake report reflects this.
  constexpr uint32_t kSignalCount = 4;
  for (uint32_t i = 0; i < kSignalCount; ++i) {
    TriggerWakeSource(0);
    AckWakeSource(0);
  }
  TriggerWakeSource(0);

  // Even though the source has been ack'ed in the past, it is still pending now
  // and therefore not in the "acked state" and will continue to be reported
  // until it finally has been acked.
  for (uint32_t i = 0; i < 2; ++i) {
    EXPECT_OK(DoSuspend(zx::time_boot::infinite()));
    EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
    EXPECT_EQ(1u, entries().size());
    VerifyWakeSourceReported(0, ExpectAcked::No);
  }

  // Now ack the source.  Since the entry for the source has been reported in
  // the past, it should go away and there should now be nothing to report.
  AckWakeSource(0);
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoDiscard() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);
  // Signal all of our sources, and ack the sources with an even index.  Then
  // request a report with the DISCARD option set.  We should get back a set of
  // wake entries for each of our even-indexed sources since the entries for the
  // already acknowledged sources should have been discarded.
  for (uint32_t i = 0; i < irq_wake_sources().size(); ++i) {
    TriggerWakeSource(i);
    if ((i & 0x1) == 0) {
      AckWakeSource(i);
    }
  }

  constexpr uint32_t kExpectedEvents = kWakeSourceCount >> 1;
  EXPECT_OK(DoSuspend(zx::time_boot::infinite(), ZX_SYSTEM_SUSPEND_OPTION_DISCARD));
  EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
  EXPECT_EQ(kExpectedEvents, entries().size());

  // Verify that the sources we ack'ed are gone, and the sources that we didn't
  // ack were reported correctly.  Go ahead and ack the sources we had not acked
  // before.
  for (uint32_t i = 0; i < irq_wake_sources().size(); ++i) {
    if ((i & 0x1) == 0) {
      EXPECT_NULL(FindReportKoid(irq_wake_sources()[i].koid_));
    } else {
      VerifyWakeSourceReported(i, ExpectAcked::No);
      AckWakeSource(i);
    }
  }

  // Everything should be ack'ed by now.  Finish up by verifying that there is
  // nothing left to report.
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoDestructionNoAck() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Trigger one of our sources, but don't ack it.  Then destroy it and verify
  // that there is nothing to report.
  TriggerWakeSource(0);
  DestroyWakeSource(0);
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoDestructionYesAck() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Trigger and ack one of our sources, then destroy it and verify that there
  // is nothing to report.
  TriggerWakeSource(0);
  AckWakeSource(0);
  DestroyWakeSource(0);
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoAlreadyTimedOut() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Attempt to suspend with no asserted wake sources, and a deadline already in
  // the past.  The system not actually suspend, but return a report containing
  // only the special deadline wake-source.  Verify this.
  //
  // Note: this is basically already what VerifyNothingToReport does, so we can
  // just call that.
  VerifyNothingToReport();
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoReportOnly() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Test the "report only" feature by calling zx_system_suspend_enter with no
  // pending wake report entries from our IRQ wake sources, and a timeout in the
  // past. Since we requested only a report, the deadline is ignored, and we
  // should immediately get back a report with zero wake report entries in it,
  // not even the timeout entry.
  EXPECT_OK(DoSuspend(zx::time_boot::infinite(), ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY));
  EXPECT_EQ(0u, report_hdr().unreported_wake_report_entries);
  EXPECT_EQ(0u, entries().size());
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoSmallReportBuffer() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Make certain that we can (eventually) retrieve information about every wake
  // entry waiting to be reported, even if our buffers are too small to hold all
  // of the results at the same time.
  //
  // Note that, as tests go, this is one is a bit tricky to write.  If there are
  // entries waiting to be reported for wake sources which are not yet
  // acknowledged, those entries will continue to be reported over, and over
  // again (as they are preventing the system from suspending).  On the other
  // hand, if all of the entries waiting to be reported have already been
  // acknowledged, then the only way out of the call to suspend is via a
  // deadline, meaning that the deadline wake source needs to show up in the
  // report as well.  Since there is no guarantee what order the wake entries
  // will be reported, it is impossible to guarantee forward progress when it
  // comes to entry reporting if we pass a buffer only large enough to hold a
  // single wake entry.  If we always pass a buffer large enough to hold two
  // entries, we should always be making forward progress, but we cannot predict
  // ahead of time how many calls it will take to eventually drain all of the
  // entries waiting to be reported.
  //
  // In the interest of keeping the test more deterministic, we make use of the
  // REPORT_ONLY option.  We will start by triggering and acknowledging all of
  // our IRQ wake sources, meaning that we have that number of sources to
  // (eventually) report.  Then we will suspend with a deadline of -infinity,
  // and no room for any wake entries (just the header).
  //
  // At this point in time, there are `Num(IrqWakeSources) + 1` wake sources
  // waiting to be reported; one for each of the IRQ sources, and one for the
  // internal deadline wake sources.  Now we call suspend with the REPORT_ONLY
  // flag set, and room for one wake entry, exactly `Num(IrqWakeSources) + 1`
  // times.  In the process, we should see wake entries for each IRQ wake source
  // we triggered and acked at the start, and the deadline wake source.  We just
  // need to verify that we saw a single entry for each of these sources.
  //
  for (uint32_t i = 0; i < irq_wake_sources().size(); ++i) {
    TriggerWakeSource(i);
    AckWakeSource(i);
  }

  // Try to suspend with a timeout in the past, provide no room for entries.
  EXPECT_EQ(ZX_OK, DoSuspend(zx::time_boot::infinite_past(), 0u, 0u));

  // Now fetch kWakeSourceCount reports with room for only a single wake report
  // entry each time, using the REPORT only flag.  We should see each source
  // (including the deadline source) reported exactly once, but in an order we
  // cannot necessarily predict or control.
  bool deadline_source_reported = false;
  for (uint32_t i = 0; i < kWakeSourceCount; ++i) {
    const uint32_t expected_pending = kWakeSourceCount - i - 1;
    EXPECT_EQ(ZX_OK,
              DoSuspend(zx::time_boot::infinite(), ZX_SYSTEM_SUSPEND_OPTION_REPORT_ONLY, 1u));
    EXPECT_EQ(expected_pending, report_hdr().unreported_wake_report_entries);
    ASSERT_EQ(1u, entries().size());

    const zx_wake_source_report_entry_t& e = entries()[0];
    WakeSource* s = FindWakeSourceKoid(e.koid);
    if (s != nullptr) {
      EXPECT_FALSE(s->has_been_reported_);
      s->has_been_reported_ = true;
    } else {
      EXPECT_EQ(kDeadlineWakeSourceKoid, e.koid);
      EXPECT_FALSE(deadline_source_reported);
      deadline_source_reported = true;
    }
  }

  EXPECT_TRUE(deadline_source_reported);
  for (const WakeSource& s : irq_wake_sources()) {
    EXPECT_TRUE(s.has_been_reported_);
  }
}

template <TestFlavor Flavor>
void WakeReportTests<Flavor>::DoMultipleSignals() {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // Trigger one of our wake sources 3 times in a row.  Since our interrupt is
  // edge triggered, this should put us in a situation where we have:
  //
  // 1) Our waiting thread unblocked (thread-bound), or a single pending packet
  //    in our port (port-bound).  Either way, the timestamps of our first
  //    trigger should have been delivered via one of these mechanisms..
  // 2) A pending interrupt in the interrupt object itself which will be sent as
  //    a packet (port-bound) or unblock a waiting thread (thread-bound), with
  //    the second trigger's timestamp once the first trigger has been
  //    acked by the user.
  // 3) Three signal events recorded in the wake report's entry for this
  //    interrupt, and initial/last signal times equal to the 1st and 3rd
  //    trigger times.
  //
  // Once we are in this situation we should be able to:
  //
  // 1) Attempt to suspend and be denied as our interrupt has not been ack'ed
  //    yet.
  // 2) Read the interrupt port packet if we are port-bound.
  // 3) Ack the interrupt.
  // 4) Attempt to suspend and be denied again.  We acked our interrupt, but it
  //    remains in the signaled state because there is another packet in flight.
  //    Our wake report should show that our event is still in the signaled
  //    state.
  // 5) Read the interrupt port packet if we are port-bound.
  // 6) Ack the interrupt.
  // 7) Attempt to suspend again, and this time succeed.  Even though we
  //    triggered the interrupt 3 times in total, only two packets will end up
  //    being sent (or our thread will be unblocked twice).  The first packet
  //    for the first trigger, and the second packet to represent all of the
  //    subsequent triggers.

  // Start by queueing our initial 3 triggers.  Don't attempt to read port
  // packets after the first trigger, we will not get any until we have ack'ed
  // the interrupt.
  const std::array trigger_times{TriggerWakeSource(0), TriggerWakeSource(0), TriggerWakeSource(0)};

  // Suspend with an infinite timeout to verify that our attempt to suspend was
  // blocked.  Also, verify that our signaled wake source shows up in the
  // report, and that is has not been considered to be ack'ed yet.
  EXPECT_OK(DoSuspend(zx::time_boot::infinite()));
  VerifyWakeSourceReported(0, ExpectAcked::No);

  // Ack our source and verify that a new port packet with the second trigger
  // time is immediately generated (or our thread is immediately unblocked).
  // Then attempt to suspend a second time, also with an infinite timeout.  Once
  // again, we should bounce off the suspend operation, and once again we should
  // see our wake source in the report, and not yet ack'ed.
  AckWakeSource(0);
  WaitForAndValidateSignal(0, trigger_times[1]);
  EXPECT_OK(DoSuspend(zx::time_boot::infinite()));
  VerifyWakeSourceReported(0, ExpectAcked::No);

  // Ack our wake source one final time and verify that no port packet is
  // generated (or that our thread remains unblocked).  At this point, there
  // should be no pending wake source to report.  As soon as we ack'ed our
  // source, it was both ack'ed and reported and should disappear from the
  // report.
  //
  // Attempt to suspend, but do so with a deadline in the past.  There is
  // nothing pending to report, so we should be "allowed to suspend", but then
  // immediately report a timeout.  If we still had a wake source waiting to be
  // reported, we would not see the timeout, just the still-waiting wake source.
  AckWakeSource(0);
  VerifyNoSignal();
  EXPECT_OK(DoSuspend(zx::time_boot::infinite_past()));
  VerifyNothingToReport();
}

// We can't use <> in the TEST_F macros, so make some aliases so we don't have to.
using WakeReportTestsPortBound = WakeReportTests<TestFlavor::PortBound>;
using WakeReportTestsThreadBound = WakeReportTests<TestFlavor::ThreadBound>;

TEST_F(WakeReportTestsPortBound, BadReportRequests) { DoBadReportRequests(); }
TEST_F(WakeReportTestsPortBound, SingleWakeSource) { DoSingleWakeSource(); }
TEST_F(WakeReportTestsPortBound, MultiWakeSource) { DoMultiWakeSource(); }
TEST_F(WakeReportTestsPortBound, Acked) { DoAcked(); }
TEST_F(WakeReportTestsPortBound, AckedStillPending) { DoAckedStillPending(); }
TEST_F(WakeReportTestsPortBound, Discard) { DoDiscard(); }
TEST_F(WakeReportTestsPortBound, DestructionNoAck) { DoDestructionNoAck(); }
TEST_F(WakeReportTestsPortBound, DestructionYesAck) { DoDestructionYesAck(); }
TEST_F(WakeReportTestsPortBound, AlreadyTimedOut) { DoAlreadyTimedOut(); }
TEST_F(WakeReportTestsPortBound, ReportOnly) { DoReportOnly(); }
TEST_F(WakeReportTestsPortBound, SmallReportBuffer) { DoSmallReportBuffer(); }
TEST_F(WakeReportTestsPortBound, MultipleSignals) { DoMultipleSignals(); }

TEST_F(WakeReportTestsThreadBound, BadReportRequests) { DoBadReportRequests(); }
TEST_F(WakeReportTestsThreadBound, SingleWakeSource) { DoSingleWakeSource(); }
TEST_F(WakeReportTestsThreadBound, MultiWakeSource) { DoMultiWakeSource(); }
TEST_F(WakeReportTestsThreadBound, Acked) { DoAcked(); }
TEST_F(WakeReportTestsThreadBound, AckedStillPending) { DoAckedStillPending(); }
TEST_F(WakeReportTestsThreadBound, Discard) { DoDiscard(); }
TEST_F(WakeReportTestsThreadBound, DestructionNoAck) { DoDestructionNoAck(); }
TEST_F(WakeReportTestsThreadBound, DestructionYesAck) { DoDestructionYesAck(); }
TEST_F(WakeReportTestsThreadBound, AlreadyTimedOut) { DoAlreadyTimedOut(); }
TEST_F(WakeReportTestsThreadBound, ReportOnly) { DoReportOnly(); }
TEST_F(WakeReportTestsThreadBound, SmallReportBuffer) { DoSmallReportBuffer(); }
TEST_F(WakeReportTestsThreadBound, MultipleSignals) { DoMultipleSignals(); }

}  // namespace
