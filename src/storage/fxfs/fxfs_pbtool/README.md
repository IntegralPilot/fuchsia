# fxfs_pbtool

A command-line tool designed to extract file blobs directly from an Fxfs sparse
image.

## Separation from //src/storage/fxfs/tools

This tool is intentionally separate from the general Fxfs tools located in
`//src/storage/fxfs/tools` because it is intended to be included within the
product bundle itself. As such, it acts as a public interface.

> [!IMPORTANT]
> Because this tool will be part of the product bundle, its API must be kept
> **stable and minimal**. Changes to the interface could break other tools
> that depend on it.

## Goals

*   **Primary Goal:** Enable the removal of the redundant `blobs/` directory in
    the product bundle. This will significantly shrink the product bundle size
    and speed up build times.
*   **Secondary Goal:** Support tools like `ffx scrutiny` and `ffx repository` in
    accessing artifacts directly from the Fxfs image, rather than relying on a
    separate extracted directory.
