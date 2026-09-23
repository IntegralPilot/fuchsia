// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/find_line.h"

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/common/address_range.h"
#include "src/developer/debug/zxdb/common/string_util.h"
#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"
#include "src/developer/debug/zxdb/symbols/dwarf_tag.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/mock_line_table.h"
#include "src/developer/debug/zxdb/symbols/mock_symbol_factory.h"
#include "src/developer/debug/zxdb/symbols/symbol_test_parent_setter.h"

namespace zxdb {

// For outputting error messages for LineMatch.
std::ostream& operator<<(std::ostream& out, const LazySymbol& s) {
  // This test only cares about the DIE offsets, the symbol factories are the same.
  return out << s.die_ref();
}
std::ostream& operator<<(std::ostream& out, const LineMatch& m) {
  return out << "LineMatch(" << to_hex_string(m.address) << ", " << m.line << ", " << m.function
             << ")";
}

TEST(FindLine, GetAllLineTableMatchesInUnit) {
  // The same file name can appear more than once as a line table "file" (they could be duplicates,
  // or they could be encoded with a different directory that still resolves to the same file).
  MockLineTable::FileNameVector files;
  files.push_back("file1.cc");  // Name for file ID #1.
  files.push_back("file2.cc");  // Name for file ID #2.
  files.push_back("file1.cc");  // Name for file ID #3 (duplicate of #1).

  MockLineTable::RowVector rows;
  rows.push_back(MockLineTable::MakeStatementRow(0x1000, 1, 1));  // File #1, line 1.
  rows.push_back(MockLineTable::MakeStatementRow(0x1001, 1, 2));
  rows.push_back(MockLineTable::MakeStatementRow(0x1002, 2, 1));  // File #2, line 1.
  rows.push_back(MockLineTable::MakeStatementRow(0x1003, 1, 1));  // Dupe for File 1, line 1.
  rows.push_back(MockLineTable::MakeStatementRow(0x1004, 1, 90));
  rows.push_back(MockLineTable::MakeStatementRow(0x1005, 1, 100));
  rows.push_back(MockLineTable::MakeStatementRow(0x1006, 3, 95));
  rows.push_back(MockLineTable::MakeStatementRow(0x1007, 3, 100));
  rows.push_back(MockLineTable::MakeStatementRow(0x1008, 1, 98));
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x1009, 1, 98));

  MockLineTable table(files, rows);
  MockSymbolFactory symbol_factory;

  constexpr uint64_t kFnBegin = 0x1000;
  constexpr uint64_t kFnEnd = 0x2000;
  auto fn1 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn1->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345678), fn1);
  table.SetSymbolForRow(rows[0], fn1);

  auto fn2 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn2->set_code_ranges(AddressRanges(AddressRange(kFnBegin + 100, kFnEnd + 100)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345778), fn2);
  table.SetSymbolForRow(rows[3], fn2);

  // There are two exact matches for line 1.
  auto out = GetAllLineTableMatchesInUnit(table, "file1.cc", 1);
  ASSERT_EQ(2u, out.size());
  EXPECT_EQ(LineMatch(0x1000, 1, fn1->GetLazySymbol()), out[0]);
  EXPECT_EQ(LineMatch(0x1003, 1, fn2->GetLazySymbol()), out[1]);

  auto fn3 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn3->set_code_ranges(AddressRanges(AddressRange(kFnBegin + 200, kFnEnd + 200)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345878), fn3);
  table.SetSymbolForRow(rows[5], fn3);

  auto fn4 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn4->set_code_ranges(AddressRanges(AddressRange(kFnBegin + 300, kFnEnd + 300)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345978), fn4);
  table.SetSymbolForRow(rows[7], fn4);

  // Searching for line 99 should catch both the 90->100 and the 95->100 transitions.
  out = GetAllLineTableMatchesInUnit(table, "file1.cc", 99);
  ASSERT_EQ(2u, out.size());
  EXPECT_EQ(LineMatch(0x1005, 100, fn3->GetLazySymbol()), out[0]);
  EXPECT_EQ(LineMatch(0x1007, 100, fn4->GetLazySymbol()), out[1]);

  // Searching for something greater than 100 should fail.
  out = GetAllLineTableMatchesInUnit(table, "file1.cc", 101);
  EXPECT_TRUE(out.empty());
}

// Out-of-order lines. In this case there was some later code moved before the line being searched
// for, even though the transition of addresses goes in the opposite direction (high to low), we
// should find the line.
TEST(FindLine, GetAllLineTableMatchesInUnit_Reverse) {
  MockLineTable::FileNameVector files = {"file1.cc"};

  MockLineTable::RowVector rows;
  rows.push_back(MockLineTable::MakeStatementRow(0x1000, 1, 105));  // Later code moved before.
  rows.push_back(MockLineTable::MakeStatementRow(0x1001, 1, 101));  // Best match.
  rows.push_back(MockLineTable::MakeStatementRow(0x1002, 1, 91));   //
  rows.push_back(MockLineTable::MakeStatementRow(0x1003, 1, 103));  // Less-good match.
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x1004, 1, 103));

  MockLineTable table(files, rows);
  MockSymbolFactory symbol_factory;

  constexpr uint64_t kFnBegin = 0x1000;
  constexpr uint64_t kFnEnd = 0x2000;
  auto fn1 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn1->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345678), fn1);
  table.SetSymbolForRow(rows[1], fn1);

  auto out = GetAllLineTableMatchesInUnit(table, "file1.cc", 100);
  ASSERT_EQ(1u, out.size());
  EXPECT_EQ(LineMatch(0x1001, 101, fn1->GetLazySymbol()), out[0]);
}

TEST(FindLine, GetAllLineTableMatchesInUnitIgnoreInvalidFunctions) {
  MockLineTable::FileNameVector files = {"file1.cc"};

  MockLineTable::RowVector rows;
  rows.push_back(MockLineTable::MakeStatementRow(0x1000, 1, 1));  // File #1, line 1.
  rows.push_back(MockLineTable::MakeStatementRow(0x1001, 1, 2));
  rows.push_back(MockLineTable::MakeStatementRow(0x1002, 1, 1));  // Dupe for file #1, line 1, but
                                                                  // has no function symbol.
  rows.push_back(MockLineTable::MakeStatementRow(0x1003, 1, 3));
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x1004, 1, 3));

  MockLineTable table(files, rows);
  MockSymbolFactory symbol_factory;

  constexpr uint64_t kFnBegin = 0x1000;
  constexpr uint64_t kFnEnd = 0x2000;
  auto fn1 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn1->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345678), fn1);
  table.SetSymbolForRow(rows[0], fn1);

  // We will only have one match, because only one row has a valid function symbol.
  auto out = GetAllLineTableMatchesInUnit(table, "file1.cc", 1);
  ASSERT_EQ(1u, out.size());
  EXPECT_EQ(LineMatch(0x1000, 1, fn1->GetLazySymbol()), out[0]);
}

TEST(FindLine, GetAllLineTableMatchesInUnitIgnoreInvalidFunctions_Reverse) {
  MockLineTable::FileNameVector files = {"file1.cc"};

  MockLineTable::RowVector rows;
  rows.push_back(MockLineTable::MakeStatementRow(0x1000, 1, 1));  // File #1, line 1.
  rows.push_back(MockLineTable::MakeStatementRow(0x1001, 1, 2));
  rows.push_back(MockLineTable::MakeStatementRow(0x1002, 1, 1));  // Dupe for file #1, line 1, but
                                                                  // this one has a function symbol.
  rows.push_back(MockLineTable::MakeStatementRow(0x1003, 1, 3));
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x1004, 1, 3));

  MockLineTable table(files, rows);
  MockSymbolFactory symbol_factory;

  constexpr uint64_t kFnBegin = 0x1000;
  constexpr uint64_t kFnEnd = 0x2000;
  auto fn1 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn1->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x12345678), fn1);
  table.SetSymbolForRow(rows[2], fn1);

  // We will only have one match, because only one row has a valid function symbol.
  auto out = GetAllLineTableMatchesInUnit(table, "file1.cc", 1);
  ASSERT_EQ(1u, out.size());
  EXPECT_EQ(LineMatch(0x1002, 1, fn1->GetLazySymbol()), out[0]);
}

TEST(FindLine, AppendLineMatchesForInlineCalls) {
  // The location we're searching for.
  const char kFilename[] = "file.cc";
  const int kLine = 100;

  // This will set the DIE offsets for the symbols we make.
  MockSymbolFactory symbol_factory;

  // The structure we're setting up is:
  //
  // DW_TAG_subprogram outer_fn
  //   DW_TAG_lexical_block outer_block
  //     DW_TAG_inlined_subroutine inline_call1 (called BEFORE query line)
  //     DW_TAG_inlined_subroutine inline_call2 (called AT query line)
  //     DW_TAG_inlined_subroutine inline_call3 (called AFTER query line)
  constexpr uint64_t kFnBegin = 0x1000;
  constexpr uint64_t kFnEnd = 0x2000;
  auto outer_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  outer_fn->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x8642345), outer_fn);

  // This block covers the whole function (just to check recursive logic).
  auto outer_block = fxl::MakeRefCounted<CodeBlock>(DwarfTag::kLexicalBlock);
  outer_block->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));

  // This inlined function is called before the line in question.
  constexpr uint64_t kInlineCall1Begin = kFnBegin + 0x100;
  constexpr uint64_t kInlineCall1End = kFnBegin + 0x200;
  auto inline_call1 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline_call1->set_code_ranges(AddressRanges(AddressRange(kInlineCall1Begin, kInlineCall1End)));
  inline_call1->set_call_line(FileLine(kFilename, kLine - 1));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x71283123), inline_call1);

  // This inlined function is called at the line in question.
  constexpr uint64_t kInlineCall2Begin = kFnBegin + 0x200;
  constexpr uint64_t kInlineCall2End = kFnBegin + 0x300;
  auto inline_call2 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline_call2->set_code_ranges(AddressRanges(AddressRange(kInlineCall2Begin, kInlineCall2End)));
  inline_call2->set_call_line(FileLine(kFilename, kLine));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x973641), inline_call2);

  // This inlined function is called after the line in question.
  constexpr uint64_t kInlineCall3Begin = kFnBegin + 0x300;
  constexpr uint64_t kInlineCall3End = kFnBegin + 0x400;
  auto inline_call3 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline_call3->set_code_ranges(AddressRanges(AddressRange(kInlineCall3Begin, kInlineCall3End)));
  inline_call3->set_call_line(FileLine(kFilename, kLine + 1));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x123612935), inline_call3);

  // Hook up the hierarchy.
  SymbolTestParentSetter call1_parent_setter(inline_call1, outer_block);
  SymbolTestParentSetter call2_parent_setter(inline_call2, outer_block);
  SymbolTestParentSetter call3_parent_setter(inline_call3, outer_block);
  outer_block->set_inner_blocks(
      {LazySymbol(inline_call1), LazySymbol(inline_call2), LazySymbol(inline_call3)});

  SymbolTestParentSetter outer_block_parent_setter(outer_block, outer_fn);
  outer_fn->set_inner_blocks({LazySymbol(outer_block)});

  std::vector<LineMatch> result;
  AppendLineMatchesForInlineCalls(outer_fn.get(), kFilename, kLine, outer_fn.get(), &result);

  // We should get only the exact match.
  ASSERT_EQ(1u, result.size());
  EXPECT_EQ(result[0], LineMatch(kInlineCall2Begin, kLine, outer_fn->GetLazySymbol())) << result[0];

  // Let's pretend the line table found another match after the line in question (this would
  // normally be the case).
  result.emplace_back(kInlineCall2Begin + 10, kLine + 1, outer_fn->GetLazySymbol());

  // GetBestLineMatches() should return only the inline match because it's an exact match.
  std::vector<LineMatch> best = GetBestLineMatches(result);
  ASSERT_EQ(1u, best.size());
  EXPECT_EQ(best[0], LineMatch(kInlineCall2Begin, kLine, outer_fn->GetLazySymbol())) << best[0];
}

// Nested inline calls can mean there is more than one match for a line in a given physical
// function. This happens if the breakpoint is requested at an given line calling an inner inline
// function nested inside an inlined function that is called more than once.
TEST(FindLine, AppendLineMatchesForInlineCalls_Multiple) {
  // The location we're searching for.
  const char kFilename[] = "file.cc";
  const int kLine = 100;

  // This will set the DIE offsets for the symbols we make.
  MockSymbolFactory symbol_factory;

  // The structure we're setting up is:
  //
  // DW_TAG_subprogram outer_fn
  //   DW_TAG_inlined_subroutine inline1_call1   (called before query line)
  //     DW_TAG_inlined_subroutine inline2_call1 (called at query line)
  //   DW_TAG_inlined_subroutine inline1_call2   (called after query line)
  //     DW_TAG_inlined_subroutine inline2_call2 (called at query line)
  constexpr uint64_t kFnBegin = 0x1000;
  constexpr uint64_t kFnEnd = 0x2000;
  auto outer_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  outer_fn->set_code_ranges(AddressRanges(AddressRange(kFnBegin, kFnEnd)));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x8642345), outer_fn);

  // First level of inline functions
  constexpr uint64_t kInline1Call1Begin = kFnBegin + 0x100;
  constexpr uint64_t kInline1Call1End = kFnBegin + 0x200;
  auto inline1_call1 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline1_call1->set_code_ranges(AddressRanges(AddressRange(kInline1Call1Begin, kInline1Call1End)));
  inline1_call1->set_call_line(FileLine(kFilename, kLine - 50));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x71283123), inline1_call1);

  constexpr uint64_t kInline1Call2Begin = kFnBegin + 0x200;
  constexpr uint64_t kInline1Call2End = kFnBegin + 0x300;
  auto inline1_call2 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline1_call2->set_code_ranges(AddressRanges(AddressRange(kInline1Call2Begin, kInline1Call2End)));
  inline1_call2->set_call_line(FileLine(kFilename, kLine + 300));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x973641), inline1_call2);

  // Second level of inlined functions (called at the query line).
  constexpr uint64_t kInline2Call1Begin = kInline1Call1Begin + 0x10;
  constexpr uint64_t kInline2Call1End = kInline2Call1Begin + 0x20;
  auto inline2_call1 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline2_call1->set_code_ranges(AddressRanges(AddressRange(kInline2Call1Begin, kInline2Call1End)));
  inline2_call1->set_call_line(FileLine(kFilename, kLine));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x123612935), inline2_call1);

  constexpr uint64_t kInline2Call2Begin = kInline1Call2Begin + 0x10;
  constexpr uint64_t kInline2Call2End = kInline2Call2Begin + 0x20;
  auto inline2_call2 = fxl::MakeRefCounted<Function>(DwarfTag::kInlinedSubroutine);
  inline2_call2->set_code_ranges(AddressRanges(AddressRange(kInline2Call2Begin, kInline2Call2End)));
  inline2_call2->set_call_line(FileLine(kFilename, kLine));
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x123612935), inline2_call2);

  // Hook up the hierarchy.
  SymbolTestParentSetter call11_parent_setter(inline1_call1, outer_fn);
  SymbolTestParentSetter call12_parent_setter(inline1_call2, outer_fn);
  outer_fn->set_inner_blocks({LazySymbol(inline1_call1), LazySymbol(inline1_call2)});

  SymbolTestParentSetter call21_parent_setter(inline2_call1, inline1_call1);
  inline1_call1->set_inner_blocks({LazySymbol(inline2_call1)});

  SymbolTestParentSetter call22_parent_setter(inline2_call2, inline1_call2);
  inline1_call2->set_inner_blocks({LazySymbol(inline2_call2)});

  std::vector<LineMatch> result;
  AppendLineMatchesForInlineCalls(outer_fn.get(), kFilename, kLine, outer_fn.get(), &result);

  // This should return the two exact matches.
  ASSERT_EQ(2u, result.size());
  EXPECT_EQ(result[0], LineMatch(kInline2Call1Begin, kLine, inline1_call1->GetLazySymbol()))
      << std::hex << result[0].function << " " << inline1_call1->GetLazySymbol();
  EXPECT_EQ(result[1], LineMatch(kInline2Call2Begin, kLine, inline1_call2->GetLazySymbol()))
      << std::hex << result[1].function << " " << inline1_call2->GetLazySymbol();

  // Both matches should be kept when ranking. The order is not important.
  std::vector<LineMatch> best = GetBestLineMatches(result);
  ASSERT_EQ(2u, best.size());
  EXPECT_TRUE((result[0] == best[0] && result[1] == best[1]) ||
              (result[0] == best[1] && result[1] == best[0]));
}

TEST(FindLine, GetBestLineMatches) {
  // Empty input.
  auto out = GetBestLineMatches({});
  EXPECT_TRUE(out.empty());

  LazySymbol fn_null;

  // Should return the smallest line #.
  out = GetBestLineMatches({LineMatch(0x1000, 10, fn_null), LineMatch(0x1001, 7, fn_null),
                            LineMatch(0x1002, 100, fn_null)});
  ASSERT_EQ(1u, out.size());
  EXPECT_EQ(LineMatch(0x1001, 7, 0), out[0]);

  // Set up some functions for the lines below.
  MockSymbolFactory symbol_factory;
  auto fn1 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x2000), fn1);
  auto fn2 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x3000), fn2);
  auto fn3 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x4000), fn3);

  // When the smallest match has dupes, all should be returned assuming the functions are different.
  out = GetBestLineMatches({LineMatch(0x1000, 10, fn_null), LineMatch(0x1001, 20, fn1),
                            LineMatch(0x1002, 10, fn2), LineMatch(0x1003, 30, fn3)});
  ASSERT_EQ(2u, out.size());
  EXPECT_EQ(LineMatch(0x1000, 10, fn_null), out[0]);
  EXPECT_EQ(LineMatch(0x1002, 10, fn2), out[1]);

  // When a non-inlined function has multiple matches across the same line we will match all unique
  // CodeBlocks returned from GetMostSpecificChild for each match.
  auto fn4 = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  symbol_factory.SetMockSymbol(DwarfDieRef::Main(0x5000), fn4);

  // The existence of multiple matches for the same function on the same line at different addresses
  // implies that it's been broken up into multiple CodeBlocks that correspond to this line, we
  // should match both.
  out = GetBestLineMatches({LineMatch(0x5010, 25, fn4), LineMatch(0x5024, 25, fn4)});

  // We should have two matches, because fn4 has discontiguous matching regions that were passed to
  // us (it's up to the caller to filter these out before passing us the results).
  ASSERT_EQ(2u, out.size());
  ASSERT_EQ(LineMatch(0x5010, 25, fn4), out[0]);
  ASSERT_EQ(LineMatch(0x5024, 25, fn4), out[1]);
}

// Tests looking for a prologue end marker that's not present.
TEST(FindLine, GetFunctionPrologueSize_NotFound) {
  MockLineTable::FileNameVector files = {"file.cc"};

  // This line table matches what's generated by GCC (which doesn't seem to generate prologue_end
  // annotations) for the code:
  //   1  #include <stdio.h>
  //   2
  //   3  void PrologueTest() { int a; scanf("%d", &a); printf("Scanned %d\n", a);
  //   4    printf("END\n");
  //   5  }
  //   6
  //   7  int main(int argc, char **argv) {
  //   8    PrologueTest();
  //   9    return 0;
  //  10  }
  MockLineTable::RowVector rows;
  // clang-format off
  rows.push_back(MockLineTable::MakeStatementRow(  0x1155, 1, 3));  // PrologueTest function start.
  rows.push_back(MockLineTable::MakeStatementRow(  0x115d, 1, 3));  // First code in function.
  rows.push_back(MockLineTable::MakeStatementRow(  0x1175, 1, 3));
  rows.push_back(MockLineTable::MakeStatementRow(  0x118b, 1, 4));
  rows.push_back(MockLineTable::MakeStatementRow(  0x1197, 1, 5));
  rows.push_back(MockLineTable::MakeStatementRow(  0x119a, 1, 7));  // main function start.
  rows.push_back(MockLineTable::MakeStatementRow(  0x11a9, 1, 8));  // First code in function.
  rows.push_back(MockLineTable::MakeStatementRow(  0x11ae, 1, 9));
  rows.push_back(MockLineTable::MakeStatementRow(  0x11b3, 1, 10));
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x11b5, 1, 10));
  // clang-format on

  MockLineTable table(files, rows);

  auto prologue_test_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  prologue_test_fn->set_code_ranges(AddressRanges(AddressRange(0x1155, 0x119a)));

  auto main_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  main_fn->set_code_ranges(AddressRanges(AddressRange(0x119a, 0x11b5)));

  // Prologue ends at 2nd line table entry (0x115d for PrologueTest(), 0x11a9 for main()).
  EXPECT_EQ(0x8u, GetFunctionPrologueSize(table, prologue_test_fn.get()));
  EXPECT_EQ(0xfu, GetFunctionPrologueSize(table, main_fn.get()));
}

// Test looking for a prologue end marker that's present.
TEST(FindLine, GetFunctionPrologueSize_Marked) {
  MockLineTable::FileNameVector files = {"file.cc"};

  // This line table is the Clang version of the same code from above. It marks the prologue end.
  // Here, we manually added an additional row that Clang didn't to push the marked prologue end
  // past the 2nd row of a function. Otherwise this case would be identical to the above.
  MockLineTable::RowVector rows;
  // clang-format off
  rows.push_back(MockLineTable::MakeStatementRow(   0x2010d0, 1, 3));  // PrologueTest fn start.
  rows.push_back(MockLineTable::MakeStatementRow(   0x2010d1, 1, 3));  // Added manually.
  rows.push_back(MockLineTable::MakePrologueEndRow( 0x2010d8, 1, 3));  // prologue_end
  rows.push_back(MockLineTable::MakeNonStatementRow(0x2010ed, 1, 3));
  rows.push_back(MockLineTable::MakeNonStatementRow(0x2010f0, 1, 3));
  rows.push_back(MockLineTable::MakeStatementRow(   0x201104, 1, 4));  // Invalid fn decl. here.
  rows.push_back(MockLineTable::MakeStatementRow(   0x201118, 1, 5));
  rows.push_back(MockLineTable::MakeStatementRow(   0x201120, 1, 7));  // main function start.
  rows.push_back(MockLineTable::MakePrologueEndRow( 0x201136, 1, 8));  // prologue_end
  rows.push_back(MockLineTable::MakeStatementRow(   0x20113d, 1, 9));
  rows.push_back(MockLineTable::MakeEndSequenceRow( 0x201143, 1, 9));
  // clang-format on

  MockLineTable table(files, rows);

  auto prologue_test_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  prologue_test_fn->set_code_ranges(AddressRanges(AddressRange(0x2010d0, 0x20111e)));

  auto main_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  main_fn->set_code_ranges(AddressRanges(AddressRange(0x201120, 0x201143)));

  // Prologue ends at 2nd line table entry (0x115d for PrologueTest(), 0x11a9 for main()).
  EXPECT_EQ(0x8u, GetFunctionPrologueSize(table, prologue_test_fn.get()));
  EXPECT_EQ(0x16u, GetFunctionPrologueSize(table, main_fn.get()));

  // Make a function declaration that consists of exactly one line. This is invalid and not
  // actually in the example code. The prologue computation code should not try to go outside of
  // this function to get the prologue, so it will return 0.
  auto invalid_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  invalid_fn->set_code_ranges(AddressRanges(AddressRange(0x201104, 0x201118)));
  EXPECT_EQ(0u, GetFunctionPrologueSize(table, invalid_fn.get()));

  // Make a function that's before the table
  auto floating_fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  floating_fn->set_code_ranges(AddressRanges(AddressRange(0x10000, 0x10010)));
  EXPECT_EQ(0u, GetFunctionPrologueSize(table, floating_fn.get()));

  // One that's after the table.
  floating_fn->set_code_ranges(AddressRanges(AddressRange(0x300000, 0x300020)));
  EXPECT_EQ(0u, GetFunctionPrologueSize(table, floating_fn.get()));
}

// A synthetic test for when the prologue end is immediately followed by compiler-synthesized code
// (marked as "line 0") that the user doesn't want to see.
TEST(FindLine, GetFunctionPrologueSize_ZeroRows) {
  MockLineTable::FileNameVector files = {"file.cc"};

  // This line table is the Clang version of the same code from above. It marks the prologue end.
  MockLineTable::RowVector rows;
  // clang-format off
  rows.push_back(MockLineTable::MakeStatementRow(  0x1155, 1, 3));  // PrologueTest function start.
  rows.push_back(MockLineTable::MakeStatementRow(  0x115d, 1, 0));  // Generated code
  rows.push_back(MockLineTable::MakeStatementRow(  0x1175, 1, 3));  // Identified first addr.
  rows.push_back(MockLineTable::MakeStatementRow(  0x118b, 1, 4));
  rows.push_back(MockLineTable::MakeStatementRow(  0x1197, 1, 5));
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x119a, 1, 5));
  // clang-format on

  MockLineTable table(files, rows);

  auto fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn->set_code_ranges(AddressRanges(AddressRange(0x1155, 0x119a)));

  // Test with no explicltly marked prologue_end.
  EXPECT_EQ(0x20u, GetFunctionPrologueSize(table, fn.get()));

  // Explicitly mark the prologue end and try again.
  rows[1].PrologueEnd = 1;
  EXPECT_EQ(0x20u, GetFunctionPrologueSize(table, fn.get()));
}

TEST(FindLine, GetFunctionPrologueSize_NoContent) {
  MockLineTable::FileNameVector files = {"file.cc"};

  // This line table was generated by Swift for an empty function.
  MockLineTable::RowVector rows;
  // clang-format off
  rows.push_back(MockLineTable::MakeStatementRow(0x55b4481aaef0, 1, 5));
  // Both PrologueEnd and EpilogueBegin on this line.
  rows.push_back(MockLineTable::MakePrologueEndRow(0x55b4481aaef4, 1, 0));
  rows.back().EpilogueBegin = 1;
  rows.push_back(MockLineTable::MakeEndSequenceRow(0x55b4481aaef6, 1, 0));
  // clang-format on

  MockLineTable table(files, rows);

  auto fn = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  fn->set_code_ranges(AddressRanges(AddressRange(0x55b4481aaef0, 0x55b4481aaef6)));

  EXPECT_EQ(0u, GetFunctionPrologueSize(table, fn.get()));
}

}  // namespace zxdb
