# Unicode NFD normalization and casefolding

This directory contains two libraries:

`generator` consumes Unicode's official UCD dataset and produces `unicode_gen.rs`.
As of June 2024, this is around 25kiBi for UCD version 12.1.0.

`unicode` makes use of this data set to provide :
  * NFD normalization
  * Case folding
  * Default ignorable character mappings.
  * A convenience comparator `CasefoldStr` that combines the above three.

## Updating unicode versions

See the update script in the scripts/ subdirectory. Updates to CIPD prebuilts will
trigger regeneration of unicode_gen.rs.
