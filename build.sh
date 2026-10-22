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
    cargo build --release --target "$TARGET" -p plexdio
    mkdir -p dist
    cp "target/$TARGET/release/plexdio.exe" dist/plexdio.exe
    cp assets/icon.ico dist/plexdio.ico
    file dist/plexdio.exe
    ls -l dist/plexdio.exe | awk '{printf "size: %.2f MB\n", $5/1048576}'
    echo "imports:"; x86_64-w64-mingw32-objdump -p dist/plexdio.exe | awk '/DLL Name/ {print "  " $3}' | sort -fu
    ;;
  *) echo "usage: $0 [dist|test]"; exit 2 ;;
esac
