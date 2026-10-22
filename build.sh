#!/usr/bin/env bash
# Reproducible cross-build of the Windows release exe into dist/.
#   ./build.sh          release build + copy to dist/
#   ./build.sh test     run the test suite (host)
set -euo pipefail
cd "$(dirname "$0")"
TARGET=x86_64-pc-windows-gnu
case "${1:-dist}" in
  test) cargo test --workspace ;;
  dist)
    cargo build --release --target "$TARGET" -p plexify
    mkdir -p dist
    cp "target/$TARGET/release/plexify.exe" dist/plexify.exe
    cp assets/icon.ico dist/plexify.ico
    file dist/plexify.exe
    ls -l dist/plexify.exe | awk '{printf "size: %.2f MB\n", $5/1048576}'
    echo "imports:"; x86_64-w64-mingw32-objdump -p dist/plexify.exe | awk '/DLL Name/ {print "  " $3}' | sort -fu
    ;;
  *) echo "usage: $0 [dist|test]"; exit 2 ;;
esac
