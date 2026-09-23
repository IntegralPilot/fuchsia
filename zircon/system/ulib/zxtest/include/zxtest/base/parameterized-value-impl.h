// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZXTEST_BASE_PARAMETERIZED_VALUE_IMPL_H_
#define ZXTEST_BASE_PARAMETERIZED_VALUE_IMPL_H_

#include <lib/fit/function.h>

#include <functional>
#include <string_view>

#include <zxtest/base/parameterized-value.h>
#include <zxtest/base/runner.h>

namespace zxtest::internal {

// Templated version of ParameterizedTestCase. This class provides the implementation for the
// interface the registry relies on.
template <typename T, typename U>
class ParameterizedTestCaseInfoImpl : public ParameterizedTestCaseInfo {
 public:
  using FixtureType = T;
  using ParamType = U;

  // Returns a collection of factories for all registered tests of a test case.
  using ParameterizedTestFactory =
      fit::function<internal::TestFactory(fit::function<const ParamType&()>)>;

  ParameterizedTestCaseInfoImpl() = default;
  explicit ParameterizedTestCaseInfoImpl(std::string_view test_case_name)
      : ParameterizedTestCaseInfo(test_case_name) {}
  ParameterizedTestCaseInfoImpl(const ParameterizedTestCaseInfoImpl&) = delete;
  ParameterizedTestCaseInfoImpl(ParameterizedTestCaseInfoImpl&&) = delete;
  ParameterizedTestCaseInfoImpl& operator=(const ParameterizedTestCaseInfoImpl&) = delete;
  ParameterizedTestCaseInfoImpl& operator=(ParameterizedTestCaseInfoImpl&&) = delete;
  ~ParameterizedTestCaseInfoImpl() override = default;

  // Returns a unique Id representing the first fixture used to instantiate this Parametrized test
  // case.
  TypeId GetFixtureId() const final { return TypeIdProvider<FixtureType>::Get(); }

  void AddInstantiation(std::string_view instantiation_name,
                        zxtest::internal::ValueProvider<ParamType>& provider,
                        const SourceLocation& location,
                        std::function<std::string(zxtest::TestParamInfo<ParamType>)> name_fn) {
    instantiation_fns_.push_back([this, provider = std::move(provider), name_fn, location,
                                  instantiation_name](Runner* runner) mutable {
      Instantiate<FixtureType, ParamType>(instantiation_name, location, provider, name_fn, runner);
    });
  }

  // Adds a test to the test case.
  template <typename TestImpl>
  void AddTest(std::string_view name, const SourceLocation& location) {
    static_assert(std::is_base_of_v<FixtureType, TestImpl>,
                  "Must inherit from the same fixture to be part of the same test case.");
    TestInfo info;
    info.name = name;
    info.location = location;
    info.factory = &TestImpl::template CreateFactory<TestImpl>;
    test_entries_.push_back(std::move(info));
  }

  // Registers all parametrized tests of this test case with |runner|.
  void RegisterTest(Runner* runner) final {
    for (auto& instantiation_fn : instantiation_fns_) {
      instantiation_fn(runner);
    }
  }

 private:
  friend class ParameterizedTestCaseInfoImplTestPeer;

  struct TestInfo {
    std::string name;
    SourceLocation location;
    ParameterizedTestFactory factory;
  };

  template <typename TestImpl, typename ValueType>
  void Instantiate(std::string_view instantiation_name, const SourceLocation& location,
                   zxtest::internal::ValueProvider<ValueType>& provider,
                   fit::function<std::string(zxtest::TestParamInfo<ValueType>)> name_fn,
                   Runner* runner) {
    for (size_t i = 0; i < provider.size(); ++i) {
      zxtest::TestParamInfo<ValueType> info(provider[i], i);
      for (const auto& test_entry : test_entries_) {
        // Add method for instantiation name as a param, and let the reporter
        // decide how to print this.
        runner->RegisterTest<FixtureType, TestImpl>(
            std::string{instantiation_name} + '/' + name(),  //
            test_entry.name + '/' + name_fn(info),           //
            test_entry.location.filename,                    //
            static_cast<int>(test_entry.location.line_number),
            test_entry.factory(
                [&provider, i]() mutable -> const ValueType& { return provider[i]; }));
      }
    }
  }

  std::vector<fit::function<void(Runner* runner)>> instantiation_fns_;
  std::vector<TestInfo> test_entries_;
  std::string name_;
};

template <typename SuiteClass, typename Type, typename TestClass>
class AddTestDelegateImpl : public AddTestDelegate {
 public:
  std::unique_ptr<ParameterizedTestCaseInfo> CreateSuite(std::string_view suite_name) final {
    return std::make_unique<ParameterizedTestCaseInfoImpl<SuiteClass, Type>>(suite_name);
  }

  bool AddTest(ParameterizedTestCaseInfo* base, const std::string_view& test_name,
               const SourceLocation& location) final {
    ZX_ASSERT_MSG(base->GetFixtureId() == TypeIdProvider<SuiteClass>::Get(),
                  "ParameterizedTestCaseInfo type must match the suite type.");
    ParameterizedTestCaseInfoImpl<SuiteClass, Type>* suite_impl =
        reinterpret_cast<ParameterizedTestCaseInfoImpl<SuiteClass, Type>*>(base);
    suite_impl->template AddTest<TestClass>(test_name, location);
    return true;
  }
};

template <typename SuiteClass, typename Type>
class AddInstantiationDelegateImpl : public AddInstantiationDelegate<Type> {
 public:
  bool AddInstantiation(ParameterizedTestCaseInfo* base, std::string_view instantiation_name,
                        const SourceLocation& location,
                        zxtest::internal::ValueProvider<Type>& provider,
                        std::function<std::string(zxtest::TestParamInfo<Type>)> name_fn) override {
    ParameterizedTestCaseInfoImpl<SuiteClass, Type>* suite_impl =
        reinterpret_cast<ParameterizedTestCaseInfoImpl<SuiteClass, Type>*>(base);
    suite_impl->AddInstantiation(instantiation_name, provider, location, name_fn);
    return true;
  }
};

}  // namespace zxtest::internal

#endif  // ZXTEST_BASE_PARAMETERIZED_VALUE_IMPL_H_
