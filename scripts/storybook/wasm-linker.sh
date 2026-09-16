#!/bin/sh
# Use the selected local linker without changing the global LLVM installation.
set -eu
exec "${ZORK_WASM_LD:-wasm-ld}" "$@"
