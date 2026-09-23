// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/sched/affine.h>
#include <lib/sched/time.h>

#include <cmath>
#include <cstdint>
#include <iostream>
#include <numeric>
#include <random>
#include <type_traits>
#include <vector>

#include <ffl/string.h>
#include <gtest/gtest.h>

namespace {

// Utility to simplify random number generation for integral value types.
class Random {
 public:
  Random() = default;

  template <typename T>
  T GetUniform(T min, T max) {
    std::uniform_int_distribution<Underlying<T>> distribution{UnderlyingType<T>::ToUnderlying(min),
                                                              UnderlyingType<T>::ToUnderlying(max)};
    return UnderlyingType<T>::FromUnderlying(distribution(generator_));
  }

 private:
  // Converts between T and the underlying integral type.
  template <typename T, typename = void>
  struct UnderlyingType;

  template <typename T>
  struct UnderlyingType<T, std::enable_if<std::is_integral_v<T>>> {
    using Integral = T;
    static constexpr T ToUnderlying(T value) { return value; }
    static constexpr T FromUnderlying(T value) { return value; }
  };
  template <typename Integral_, size_t FractionalBits>
  struct UnderlyingType<ffl::Fixed<Integral_, FractionalBits>> {
    using Integral = Integral_;
    using Value = ffl::Fixed<Integral, FractionalBits>;
    static constexpr Integral ToUnderlying(Value value) { return value.raw_value(); }
    static constexpr Value FromUnderlying(Integral value) { return Value::FromRaw(value); }
  };
  template <typename T>
  using Underlying = typename UnderlyingType<T>::Integral;

  std::mt19937_64 generator_{std::random_device{}()};
};

template <size_t PrecisionBits>
std::ostream& operator<<(std::ostream& stream, const sched::Affine<PrecisionBits>& affine) {
  return stream << "Affine<" << PrecisionBits << ">{mono_ref=" << affine.monotonic_reference_time()
                << ", var_ref=" << affine.variable_reference_time() << ", slope=" << affine.slope()
                << "}";
}

template <size_t PrecisionBits>
void DoTest() {
  const int64_t ErrorLimit = int64_t{1} << PrecisionBits;
  const int64_t StandardDeviationLimit = ErrorLimit * 341 / 1000;  // 1 sigma = 34.1%.

  using Affine = sched::Affine<PrecisionBits>;
  Random random;
  Affine affine;

  EXPECT_EQ(affine.slope(), Affine::kMaxSlope);
  EXPECT_EQ(affine.monotonic_reference_time(), 0);
  EXPECT_EQ(affine.variable_reference_time(), 0);

  const sched::Time min_time = sched::Time::Min();
  const sched::Time max_time = sched::Time::Max();

  // The timelines should be identical in the initial state.
  EXPECT_EQ(affine.MonotonicToVariable(min_time), min_time);
  EXPECT_EQ(affine.MonotonicToVariable(max_time), max_time);

  for (size_t i = 0; i < 10000; i++) {
    sched::Time x = random.GetUniform(min_time, max_time);

    EXPECT_EQ(affine.MonotonicToVariable(x), x);
    EXPECT_EQ(affine.VariableToMonotonic(x), x);
  }

  // Divide the interval [0, max_time] into equal sub-intervals and randomly choose one new
  // reference time and slope to test in each successive interval.
  const size_t interval_count = 10000;
  const size_t samples_per_interval = 10;
  const size_t max_samples = interval_count * samples_per_interval;
  const sched::Duration interval_size = max_time / interval_count;

  struct Sample {
    int64_t error;
    int64_t monotonic;
    Affine affine;

    operator int64_t() const { return error; }
  };
  std::vector<Sample> errors;
  errors.reserve(max_samples);

  sched::Time interval_start{0};
  for (size_t i = 0; i < interval_count; i++, interval_start += interval_size) {
    const sched::Time interval_end = interval_start + interval_size - 1;
    const sched::Time reference_time = random.GetUniform(interval_start, interval_end);
    EXPECT_GE(reference_time, interval_start);
    EXPECT_LE(reference_time, interval_end);

    const typename Affine::Slope slope = random.GetUniform(Affine::kMinSlope, Affine::kMaxSlope);
    EXPECT_GE(slope, Affine::kMinSlope);
    EXPECT_LE(slope, Affine::kMaxSlope);

    sched::Affine previous_affine = affine;
    affine.ChangeSlopeAtMonotonicTime(reference_time, slope);

    EXPECT_EQ(previous_affine.MonotonicToVariable(reference_time),
              affine.MonotonicToVariable(reference_time));

    for (size_t j = 0; j < samples_per_interval; j++) {
      const sched::Time monotonic = random.GetUniform(reference_time, interval_end);
      const sched::Time variable_from_monotonic = affine.MonotonicToVariable(monotonic);
      const sched::Time monotonic_from_variable =
          affine.VariableToMonotonic(variable_from_monotonic);
      const sched::Duration error = monotonic_from_variable - monotonic;

      errors.emplace_back(std::abs(error.raw_value()), monotonic.raw_value(), affine);
    }
  }

  const int64_t zero = 0;
  const int64_t samples = static_cast<int64_t>(errors.size());
  const int64_t max_error = *std::max_element(errors.cbegin(), errors.cend());

  // Simple mean.
  const int64_t sum_errors = std::accumulate(errors.cbegin(), errors.cend(), zero);
  const int64_t mean_error = sum_errors / samples;

  // Variance and standard deviation.
  const int64_t sum_squared_deviations = std::accumulate(
      errors.cbegin(), errors.cend(), zero, [mean_error](int64_t accum, int64_t error) {
        return accum + (error - mean_error) * (error - mean_error);
      });
  const int64_t variance = sum_squared_deviations / samples;
  const int64_t standard_deviation = static_cast<int64_t>(std::sqrt(variance));

  std::cout << "\nprecision_bits=" << PrecisionBits << " mean_error=" << mean_error
            << " variance=" << variance << " standard_deviation=" << standard_deviation
            << " max_error=" << max_error << "\n";

  EXPECT_LE(max_error, ErrorLimit);
  EXPECT_LE(mean_error, ErrorLimit * 60 / 100);  // 60%.
  EXPECT_LE(standard_deviation, StandardDeviationLimit);

  // Select percentiles.
  std::sort(errors.begin(), errors.end());

  const size_t percentiles[] = {999, 990, 900, 750, 600, 500, 250};
  for (const size_t percentile : percentiles) {
    using Numerator = ffl::Fixed<size_t, 2>;
    const size_t ordinal_rank = ffl::Round<size_t>(Numerator{percentile * errors.size()} / 1000);
    const Sample element = errors[ordinal_rank];
    std::cout << "\t" << (static_cast<double>(percentile) / 10) << "th: " << element.error
              << " monotonic=" << element.monotonic << " " << element.affine << "\n";

    // Check the 99.9th percentile against the test limit.
    if (percentile == 999) {
      EXPECT_LE(element.error, ErrorLimit);
    }
  }
}

TEST(AffineTests, Properties) {
  DoTest<8>();
  DoTest<9>();
  DoTest<10>();
  DoTest<11>();
  DoTest<12>();
}

}  // anonymous namespace
