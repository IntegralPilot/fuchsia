// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/fxt/serializer.h>
#include <lib/page/size.h>
#include <lib/thread_sampler/thread_sampler.h>
#include <lib/unittest/unittest.h>
#include <lib/unittest/user_memory.h>
#include <lib/user_copy/user_ptr.h>
#include <lib/zx/time.h>

#include <kernel/cpu.h>
#include <kernel/dpc.h>
#include <kernel/mp.h>
#include <kernel/scheduler.h>
#include <ktl/algorithm.h>
#include <ktl/limits.h>
#include <ktl/unique_ptr.h>
#include <vm/vm_aspace.h>

#include <ktl/enforce.h>

namespace thread_sampler_tests {

// A test version of ThreadSampler which overrides functions
// for testing purposes.
//
// Despite having a test class, we use the global sampler::gThreadSampler for tests. The timer
// callbacks sampling sets assume a static lifetime for the sampler.
class TestThreadSampler {
 public:
  static auto& get_per_cpu_buffers(sampler::ThreadSampler& sampler) {
    return sampler.per_cpu_buffers_;
  }
  static auto& get_state(sampler::ThreadSampler& sampler) { return sampler.state_; }
  static void set_state(sampler::ThreadSampler& sampler,
                        sampler::SamplingState s) TA_NO_THREAD_SAFETY_ANALYSIS {
    sampler.SetState(s);
  }
  static auto get_lock() { return sampler::ThreadSampler::ThreadSamplerLock::Get(); }

  static uint64_t get_buffer_ref_count(const sampler::ThreadSampler& sampler) {
    uint64_t state = sampler.state_.load(ktl::memory_order_relaxed);
    return (state & sampler::ThreadSampler::kBufferRefCountMask) >>
           sampler::ThreadSampler::kBufferRefCountShift;
  }

  static uint64_t get_timer_ref_count(const sampler::ThreadSampler& sampler) {
    uint64_t state = sampler.state_.load(ktl::memory_order_relaxed);
    return (state & sampler::ThreadSampler::kMarkingsScheduledMask) >>
           sampler::ThreadSampler::kMarkingsScheduledShift;
  }

  static void SampleThread(sampler::ThreadSampler& sampler, zx_koid_t pid, zx_koid_t tid,
                           GeneralRegsSource source, void* gregs) {
    ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> buffers =
        sampler.GetBufferRefForWriting(arch_curr_cpu_num());
    if (!buffers) {
      return;
    }
    percpu_writer::Buffer& cpu_state = buffers->Get();

    constexpr size_t kMaxUserBacktraceSize = 64;
    vaddr_t bt[kMaxUserBacktraceSize]{};
    for (unsigned i = 0; i < kMaxUserBacktraceSize; ++i) {
      bt[i] = i;
    }

    const fxt::ThreadRef current_thread{pid, tid};
    fxt::WriteProfilerBacktraceRecord(&cpu_state, current_mono_ticks(), current_thread, bt,
                                      sizeof(uint64_t) * kMaxUserBacktraceSize);
  }

  static bool RepeatStartStopTest() {
    BEGIN_TEST;
    {
      // Construct a thread sampler state and initialize it
      zx_sampler_config_t config{
          .period = zx::msec(1).get(),
          .buffer_size = kPageSize,
      };
      for (int i = 0; i < 10; i++) {
        zx::result<> read_handle = sampler::gThreadSampler.SetUp(config);
        ASSERT_TRUE(read_handle.is_ok());
        ASSERT_TRUE(sampler::gThreadSampler.Start().is_ok());
        ASSERT_TRUE(sampler::gThreadSampler.Stop().is_ok());
        ASSERT_TRUE(sampler::gThreadSampler.Destroy().is_ok());
      }
    }

    END_TEST;
  }
  static bool WriteSampleTest() {
    BEGIN_TEST;
    {
      // Construct a thread sampler state and initialize it
      zx_sampler_config_t config{
          .period = zx::msec(1).get(),
          .buffer_size = kPageSize,
      };
      sampler::ThreadSampler& test_state = sampler::gThreadSampler;
      ASSERT_OK(test_state.SetUp(config).status_value());

      ASSERT_OK(test_state.Start().status_value());

      zx_instant_mono_ticks_t before = current_mono_ticks();
      //  Write some fake samples to each buffer on each cpu
      mp_sync_exec(
          mp_ipi_target::ALL, 0,
          [](void* s) {
            auto& sampler = *static_cast<sampler::ThreadSampler*>(s);
            TestThreadSampler::SampleThread(sampler, arch_curr_cpu_num(), 1,
                                            GeneralRegsSource::None, nullptr);
          },
          &test_state);
      zx_instant_mono_ticks_t after = current_mono_ticks();
      ASSERT_OK(test_state.Stop().status_value());

      // We should now be able to read the records
      size_t num_cpus = arch_max_num_cpus();
      for (unsigned i = 0; i < num_cpus; ++i) {
        percpu_writer::Buffer& s = TestThreadSampler::get_per_cpu_buffers(test_state)[i];

        // num_words = 64 backtrace + 1 header + 1 ts + 1 pid + 1 tid = 68;
        constexpr size_t num_words = 68;
        const fxt::ThreadRef sample_thread{i, 1};
        const uint64_t expected_header =
            fxt::MakeHeader(fxt::RecordType::kProfiler, fxt::WordSize(num_words)) |
            fxt::ProfilerRecordFields::ProfilerRecordType::Make(
                ToUnderlyingType(fxt::ProfilerRecordType::kBacktrace)) |
            fxt::ProfilerRecordFields::ThreadRef::Make(sample_thread.HeaderEntry()) |
            fxt::ProfilerBacktraceRecordFields::BacktraceDataLength::Make(64);
        uint64_t record[68];
        auto copy_fn = [&record](uint32_t offset,
                                 ktl::span<ktl::byte> data) mutable -> zx_status_t {
          ktl::ranges::copy(data, reinterpret_cast<ktl::byte*>(record) + offset);
          return ZX_OK;
        };
        zx::result<size_t> read_result = s.Read(copy_fn, sizeof(record));
        ASSERT_TRUE(read_result.is_ok());
        // We should only get the bytes of the record we wrote.
        ASSERT_EQ(*read_result, size_t{68 * sizeof(uint64_t)});

        EXPECT_EQ(expected_header, record[0]);

        // timestamp
        EXPECT_GE(record[1], static_cast<uint64_t>(before));
        EXPECT_LE(record[1], static_cast<uint64_t>(after));

        // We wrote the cpu number as the pid
        EXPECT_EQ(i, record[2]);
        // And 1 as the tid
        EXPECT_EQ(uint64_t{1}, record[3]);
        for (unsigned frame = 0; frame < 64; frame++) {
          EXPECT_EQ(record[4 + frame], frame);
        }
      }
      ASSERT_OK(test_state.Destroy().status_value());
    }

    END_TEST;
  }

  static bool StateChange() {
    BEGIN_TEST;
    {
      sampler::ThreadSampler& sampler = sampler::gThreadSampler;
      ASSERT_EQ(uint64_t{0}, TestThreadSampler::get_state(sampler).load(ktl::memory_order_relaxed));
      zx_sampler_config_t config{
          .period = zx::msec(1).get(),
          .buffer_size = kPageSize,
      };
      ASSERT_OK(sampler.SetUp(config).status_value());
      ASSERT_EQ(sampler::SamplingState::Configured, sampler.State());
      ASSERT_TRUE(sampler.Start().is_ok());
      ASSERT_EQ(sampler::SamplingState::Running, sampler.State());
      ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> ref =
          sampler.GetBufferRefForWriting(0);
      ASSERT_TRUE(ref.has_value());
      ASSERT_EQ(sampler::SamplingState::Running, sampler.State());
      uint64_t ref_count = TestThreadSampler::get_buffer_ref_count(sampler);
      ASSERT_EQ(uint64_t{1}, ref_count);
      // Changing the state shouldn't change the ref count
      {
        Guard<Mutex> guard(TestThreadSampler::get_lock());
        TestThreadSampler::set_state(sampler, sampler::SamplingState::Stopping);
      }
      ASSERT_EQ(sampler::SamplingState::Stopping, sampler.State());
      uint64_t ref_count2 = TestThreadSampler::get_buffer_ref_count(sampler);
      ASSERT_EQ(uint64_t{1}, ref_count2);
      ref.reset();
      // Restore state to Running so StopLocked() runs during Destroy() to cancel active timers.
      {
        Guard<Mutex> guard(TestThreadSampler::get_lock());
        TestThreadSampler::set_state(sampler, sampler::SamplingState::Running);
      }
      ASSERT_OK(sampler.Destroy().status_value());
    }

    END_TEST;
  }

  static bool AcquireBuffers() {
    BEGIN_TEST;
    {
      sampler::ThreadSampler& sampler = sampler::gThreadSampler;
      // We shouldn't be able to get a buffer reference if we don't have buffers.
      for (cpu_num_t i = 0; i < arch_max_num_cpus() + 1; i++) {
        ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> ref =
            sampler.GetBufferRefForWriting(i);
        ASSERT_FALSE(ref.has_value());
      }
      // Construct a thread sampler state and initialize it
      zx_sampler_config_t config{
          .period = zx::msec(1).get(),
          .buffer_size = kPageSize,
      };
      ASSERT_OK(sampler.SetUp(config).status_value());

      // We shouldn't be able to get the buffers unless we're running.
      for (cpu_num_t i = 0; i < arch_max_num_cpus() + 1; i++) {
        ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> ref =
            sampler.GetBufferRefForWriting(i);
        ASSERT_FALSE(ref.has_value());
      }
      ASSERT_TRUE(sampler.Start().is_ok());

      for (cpu_num_t i = 0; i < arch_max_num_cpus(); i++) {
        ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> ref =
            sampler.GetBufferRefForWriting(i);
        ASSERT_TRUE(ref.has_value());
      }

      ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> bad_ref =
          sampler.GetBufferRefForWriting(arch_max_num_cpus());
      ASSERT_FALSE(bad_ref.has_value());

      ASSERT_TRUE(sampler.Stop().is_ok());
      for (cpu_num_t i = 0; i < arch_max_num_cpus() + 1; i++) {
        ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> ref =
            sampler.GetBufferRefForWriting(i);
        ASSERT_FALSE(ref.has_value());
      }
      ASSERT_TRUE(sampler.Destroy().is_ok());
      for (cpu_num_t i = 0; i < arch_max_num_cpus() + 1; i++) {
        ktl::optional<sampler::ThreadSampler::PerCpuBufferRef> ref =
            sampler.GetBufferRefForWriting(i);
        ASSERT_FALSE(ref.has_value());
      }
    }

    END_TEST;
  }

  static bool TimerReferences() {
    BEGIN_TEST;
    sampler::ThreadSampler& test_state = sampler::gThreadSampler;
    unsigned num_cpus_online = ktl::popcount(mp_get_online_mask());
    // Construct a thread sampler state and initialize it
    zx_sampler_config_t config{
        .period = zx::msec(1).get(),
        .buffer_size = kPageSize,
    };
    ASSERT_OK(test_state.SetUp(config).status_value());

    {
      // There should be no timers running yet
      ASSERT_EQ(uint64_t{0}, TestThreadSampler::get_timer_ref_count(test_state));
    }

    ASSERT_OK(test_state.Start().status_value());
    {
      // We should have a reference open for each cpu.
      ASSERT_EQ(num_cpus_online, TestThreadSampler::get_timer_ref_count(test_state));
    }

    ASSERT_OK(test_state.Stop().status_value());
    {
      // All timers should have stopped.
      ASSERT_EQ(uint64_t{0}, TestThreadSampler::get_timer_ref_count(test_state));
    }
    ASSERT_OK(test_state.Destroy().status_value());

    END_TEST;
  }

  static zx_status_t CheckTimerRefsIs(sampler::ThreadSampler& test_state, size_t expected) {
    zx_instant_mono_t end_time = zx_time_add_duration(current_mono_time(), ZX_SEC(5));
    while (current_mono_time() < end_time) {
      if (expected == TestThreadSampler::get_timer_ref_count(test_state)) {
        return ZX_OK;
      }
      Thread::Current::SleepRelative(ZX_USEC(200));
    }
    return ZX_ERR_TIMED_OUT;
  }

  static zx_status_t wait_for_cpu_offline(cpu_num_t i) {
    while (true) {
      zx::result<power_cpu_state> res = platform_get_cpu_state(i);
      if (res.is_error()) {
        if (res.error_value() == ZX_ERR_NOT_SUPPORTED) {
          // x86 does not implement platform_get_cpu_state, so return OK if the call returns
          // ZX_ERR_NOT_SUPPORTED.
          return ZX_OK;
        }
        return res.error_value();
      } else if (res.value() == power_cpu_state::OFF || res.value() == power_cpu_state::STOPPED) {
        return ZX_OK;
      }
      Thread::Current::SleepRelative(ZX_USEC(200));
    }
  }

  static zx_status_t UnplugCpus(cpu_mask_t cpumask) {
    zx_status_t res = mp_unplug_cpu_mask(cpumask, ZX_TIME_INFINITE);
    if (res != ZX_OK) {
      return res;
    }
    cpu_mask_t remaining = cpumask;
    cpu_num_t cpu_id;
    while ((cpu_id = remove_cpu_from_mask(remaining)) != INVALID_CPU) {
      wait_for_cpu_offline(cpu_id);
    }
    return ZX_OK;
  }

  static zx_status_t PlugCpus(cpu_mask_t cpumask) {
    cpu_mask_t remaining = cpumask;
    cpu_num_t cpu_id;
    while ((cpu_id = remove_cpu_from_mask(remaining)) != INVALID_CPU) {
      if (zx_status_t res = mp_hotplug_cpu_mask(cpu_num_to_mask(cpu_id)); res != ZX_OK) {
        return res;
      }
      while (!Scheduler::PeekIsActive(cpu_id)) {
        Thread::Current::SleepRelative(ZX_USEC(200));
      }

      // Create a thread, affine it to the core and join it. This will block until threads are ready
      // on the core.
      cpu_num_t running_core{INVALID_CPU};
      Thread* barrier = Thread::Create(
          "barrier-thread", +[](void* arg) { return 0; }, &running_core, DEFAULT_PRIORITY);
      if (barrier == nullptr) {
        return ZX_ERR_BAD_STATE;
      }
      barrier->SetCpuAffinity(cpu_num_to_mask(cpu_id));
      barrier->SetMigrateFn([](auto...) {});
      barrier->Resume();
      if (zx_status_t res = barrier->Join(nullptr, ZX_TIME_INFINITE); res != ZX_OK) {
        return res;
      }

      // Queue and wait for a dpc on each of the dpc queues on the core to block until dpcs are
      // ready.
      const auto restore_affinity =
          fit::defer([previous_affinity = Thread::Current::SetCpuAffinity(cpu_num_to_mask(cpu_id))](
                         void) { Thread::Current::SetCpuAffinity(previous_affinity); });
      Event event_general;
      Dpc dpc_general(
          +[](Dpc* d) {
            auto* e = d->arg<Event>();
            e->Signal();
          },
          &event_general);

      Event event_low_latency;
      Dpc dpc_low_latency(
          +[](Dpc* d) {
            auto* e = d->arg<Event>();
            e->Signal();
          },
          &event_low_latency);

      if (zx_status_t res = DpcRunner::Enqueue(dpc_general, DpcRunner::QueueType::General);
          res != ZX_OK) {
        return res;
      }

      if (zx_status_t res = DpcRunner::Enqueue(dpc_low_latency, DpcRunner::QueueType::LowLatency);
          res != ZX_OK) {
        return res;
      }

      event_general.Wait(Deadline::infinite());
      event_low_latency.Wait(Deadline::infinite());
    }
    return ZX_OK;
  }

  static bool CpuOfflining() {
    BEGIN_TEST;
#if defined(__riscv)
    printf("skipping test sampler cpu hotplug test, hotplug only suported on x64 and arm64\n");
    END_TEST;
#endif
    unsigned num_cpus_online = ktl::popcount(mp_get_online_mask());
    if (num_cpus_online < 2) {
      printf("skipping test sampler cpu offlining test, not enough online cpus\n");
      END_TEST;
    }
    Thread::Current::MigrateToCpu(BOOT_CPU_ID);
    zx_sampler_config_t config{
        .period = zx::msec(1).get(),
        .buffer_size = kPageSize,
    };
    sampler::ThreadSampler& test_state = sampler::gThreadSampler;
    cpu_mask_t original_online_mask = mp_get_online_mask();
    cpu_mask_t secondary_cpus = original_online_mask & ~cpu_num_to_mask(BOOT_CPU_ID);

    // Unplug a cpu part way through sampling
    {
      ASSERT_OK(test_state.SetUp(config).status_value());
      ASSERT_OK(test_state.Start().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, num_cpus_online));

      ASSERT_OK(UnplugCpus(secondary_cpus));
      ASSERT_OK(CheckTimerRefsIs(test_state, 1));

      ASSERT_OK(PlugCpus(secondary_cpus));
      ASSERT_OK(CheckTimerRefsIs(test_state, num_cpus_online));

      ASSERT_OK(test_state.Stop().status_value());
      ASSERT_OK(test_state.Destroy().status_value());
    }
    // Start the session with cpus missing
    {
      // Join after setting up
      ASSERT_OK(UnplugCpus(secondary_cpus));
      ASSERT_OK(test_state.SetUp(config).status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));

      ASSERT_OK(PlugCpus(secondary_cpus));
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
      ASSERT_OK(test_state.Start().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, num_cpus_online));
      ASSERT_OK(test_state.Stop().status_value());
      ASSERT_OK(test_state.Destroy().status_value());

      // Joining after starting
      ASSERT_OK(UnplugCpus(secondary_cpus));
      ASSERT_OK(test_state.SetUp(config).status_value());
      ASSERT_OK(test_state.Start().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 1));

      ASSERT_OK(PlugCpus(secondary_cpus));
      ASSERT_OK(CheckTimerRefsIs(test_state, num_cpus_online));
      ASSERT_OK(test_state.Stop().status_value());
      ASSERT_OK(test_state.Destroy().status_value());
    }

    // Stop the session with cpus missing
    {
      // Stop while missing cpus
      ASSERT_OK(test_state.SetUp(config).status_value());
      ASSERT_OK(test_state.Start().status_value());
      ASSERT_OK(UnplugCpus(secondary_cpus));

      ASSERT_OK(test_state.Stop().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));

      ASSERT_OK(PlugCpus(secondary_cpus));
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
      ASSERT_OK(test_state.Destroy().status_value());

      // Plugging after the session is destroyed
      ASSERT_OK(test_state.SetUp(config).status_value());
      ASSERT_OK(test_state.Start().status_value());
      ASSERT_OK(UnplugCpus(secondary_cpus));

      ASSERT_OK(test_state.Stop().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
      ASSERT_OK(test_state.Destroy().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));

      ASSERT_OK(PlugCpus(secondary_cpus));
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
    }

    END_TEST;
  }

  static bool CpuHotplugMultiThreaded() {
    BEGIN_TEST;
#if defined(__riscv)
    printf("skipping test sampler cpu hotplug test, hotplug only suported on x64 and arm64\n");
    END_TEST;
#endif
    cpu_mask_t original_online_mask = mp_get_online_mask();
    unsigned num_cpus_online = ktl::popcount(original_online_mask);
    if (num_cpus_online < 2) {
      printf("skipping test sampler cpu hotplug race test, not enough online cpus\n");
      END_TEST;
    }
    zx_sampler_config_t config{
        .period = zx::msec(1).get(),
        .buffer_size = kPageSize,
    };
    sampler::ThreadSampler& test_state = sampler::gThreadSampler;

    // Flash on and off the other cpus while sampling.
    ktl::atomic<bool> done{false};
    Thread* unplug_helper = Thread::Create(
        "unplug-helper",
        +[](void* arg) {
          Thread::Current::MigrateToCpu(BOOT_CPU_ID);
          cpu_mask_t secondary_cpus = mp_get_online_mask() & ~cpu_num_to_mask(BOOT_CPU_ID);
          auto* done = static_cast<ktl::atomic<bool>*>(arg);
          while (!done->load()) {
            zx_status_t status = UnplugCpus(secondary_cpus);
            if (status != ZX_OK) {
              return status;
            }
            Thread::Current::SleepRelative(ZX_MSEC(1));
            status = PlugCpus(secondary_cpus);
            if (status != ZX_OK) {
              return status;
            }
            Thread::Current::SleepRelative(ZX_MSEC(1));
          }
          return 0;
        },
        &done, DEFAULT_PRIORITY);
    ASSERT_NE(nullptr, unplug_helper);

    unplug_helper->Resume();

    // Stress start/stop
    for (int i = 0; i < 10; i++) {
      ASSERT_OK(test_state.SetUp(config).status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
      ASSERT_OK(test_state.Start().status_value());
      Thread::Current::SleepRelative(ZX_MSEC(5));
      ASSERT_OK(test_state.Stop().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
      ASSERT_OK(test_state.Destroy().status_value());
      ASSERT_OK(CheckTimerRefsIs(test_state, 0));
    }

    done.store(true);
    int thread_result = 0;
    ASSERT_OK(unplug_helper->Join(&thread_result, ZX_TIME_INFINITE));
    ASSERT_EQ(0, thread_result);

    END_TEST;
  }

  static bool AcquireBuffersReading() {
    BEGIN_TEST;
    sampler::ThreadSampler& sampler = sampler::gThreadSampler;
    // We shouldn't be able to get a buffer reference if we don't have buffers.
    {
      ktl::optional<sampler::ThreadSampler::ReadToken> ref = sampler.GetBufferRefForReading();
      ASSERT_FALSE(ref.has_value());
    }

    // Construct a thread sampler state and initialize it
    zx_sampler_config_t config{
        .period = zx::msec(1).get(),
        .buffer_size = kPageSize,
    };
    ASSERT_OK(sampler.SetUp(config).status_value());

    // Unlike writing, we can get buffer references as long as we have buffers
    {
      ktl::optional<sampler::ThreadSampler::ReadToken> ref = sampler.GetBufferRefForReading();
      ASSERT_TRUE(ref.has_value());
    }

    ASSERT_OK(sampler.Start().status_value());
    {
      ktl::optional<sampler::ThreadSampler::ReadToken> ref = sampler.GetBufferRefForReading();
      ASSERT_TRUE(ref.has_value());
    }

    ASSERT_OK(sampler.Stop().status_value());
    {
      ktl::optional<sampler::ThreadSampler::ReadToken> ref = sampler.GetBufferRefForReading();
      ASSERT_TRUE(ref.has_value());
    }

    // But once the buffers are destroyed, we can not longer get them.
    ASSERT_OK(sampler.Destroy().status_value());
    ktl::optional<sampler::ThreadSampler::ReadToken> ref = sampler.GetBufferRefForReading();
    ASSERT_FALSE(ref.has_value());

    END_TEST;
  }
  static bool ReadingDuringSession() {
    BEGIN_TEST;
    // Construct a thread sampler state and initialize it
    zx_sampler_config_t config{
        .period = zx::msec(1).get(),
        .buffer_size = kPageSize,
    };
    sampler::ThreadSampler& sampler = sampler::gThreadSampler;
    ASSERT_OK(sampler.SetUp(config).status_value());
    ASSERT_OK(sampler.Start().status_value());

    zx_instant_mono_ticks_t before = current_mono_ticks();
    //  Write some fake samples to each buffer on each cpu
    mp_sync_exec(
        mp_ipi_target::ALL, 0,
        [](void*) {
          TestThreadSampler::SampleThread(sampler::gThreadSampler, arch_curr_cpu_num(), 1,
                                          GeneralRegsSource::None, nullptr);
        },
        nullptr);
    zx_instant_mono_ticks_t after = current_mono_ticks();

    // num_words = 64 backtrace + 1 header + 1 ts + 1 pid + 1 tid = 68;
    constexpr size_t num_words = 68;

    auto [query_status, expected_size] = sampler.ReadUser(make_user_out_ptr<void>(nullptr), 0);
    ASSERT_OK(query_status);
    auto user = testing::UserMemory::Create(expected_size);
    auto [status, bytes_read] = sampler.ReadUser(user->user_out<void>(), expected_size);
    ASSERT_OK(status);

    unsigned num_cpus_online = ktl::popcount(mp_get_online_mask());
    ASSERT_EQ(num_cpus_online * size_t{68 * sizeof(uint64_t)}, bytes_read);

    for (size_t i = 0; i < num_cpus_online; i++) {
      uint64_t record[68];
      size_t record_size = sizeof(record);
      user->VmoRead(record, i * record_size, record_size);

      const fxt::ThreadRef sample_thread{i, 1};
      const uint64_t expected_header =
          fxt::MakeHeader(fxt::RecordType::kProfiler, fxt::WordSize(num_words)) |
          fxt::ProfilerRecordFields::ProfilerRecordType::Make(
              ToUnderlyingType(fxt::ProfilerRecordType::kBacktrace)) |
          fxt::ProfilerRecordFields::ThreadRef::Make(sample_thread.HeaderEntry()) |
          fxt::ProfilerBacktraceRecordFields::BacktraceDataLength::Make(64);

      EXPECT_EQ(expected_header, record[0]);

      // timestamp
      EXPECT_GE(record[1], static_cast<uint64_t>(before));
      EXPECT_LE(record[1], static_cast<uint64_t>(after));

      // We wrote the cpu number as the pid
      EXPECT_EQ(i, record[2]);
      // And 1 as the tid
      EXPECT_EQ(uint64_t{1}, record[3]);
      for (unsigned frame = 0; frame < 64; frame++) {
        EXPECT_EQ(record[4 + frame], frame);
      }
    }

    // Reading again, we should get no data
    auto [empty_status, empty_read] = sampler.ReadUser(user->user_out<void>(), expected_size);
    ASSERT_OK(empty_status);
    ASSERT_EQ(size_t{0}, empty_read);

    ASSERT_OK(sampler.Stop().status_value());
    ASSERT_OK(sampler.Destroy().status_value());

    END_TEST;
  }
};
}  // namespace thread_sampler_tests

UNITTEST_START_TESTCASE(thread_sampler_tests)
UNITTEST("init/start", thread_sampler_tests::TestThreadSampler::RepeatStartStopTest)
UNITTEST("read/write", thread_sampler_tests::TestThreadSampler::WriteSampleTest)
UNITTEST("state_change", thread_sampler_tests::TestThreadSampler::StateChange)
UNITTEST("acquire_buffers", thread_sampler_tests::TestThreadSampler::AcquireBuffers)
UNITTEST("timer_references", thread_sampler_tests::TestThreadSampler::TimerReferences)
// TODO(b/511205186): Re-enable these once hotplugging flakes have been fixed.
// UNITTEST("cpu_offlining", thread_sampler_tests::TestThreadSampler::CpuOfflining)
// UNITTEST("cpu_hotplug_multithreaded",
//          thread_sampler_tests::TestThreadSampler::CpuHotplugMultiThreaded)
UNITTEST("acquire_buffers_reading", thread_sampler_tests::TestThreadSampler::AcquireBuffersReading)
UNITTEST("reading_during_session", thread_sampler_tests::TestThreadSampler::ReadingDuringSession)
UNITTEST_END_TESTCASE(thread_sampler_tests, "thread_sampler", "Thread Sampler tests")
