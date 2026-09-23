/*
 * Discard FatLTO sections in relocatable links, since the linker treats the
 * bitcode section as an orphan section and concatenates all the `.llvm.lto`
 * sections together. Alternatively, we can discard based on SHT_LLVM_LTO.
 */
SECTIONS { /DISCARD/ : { *(.llvm.lto) } }
