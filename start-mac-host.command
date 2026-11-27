#!/bin/bash
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
BIN="$DIR/target/release/treelink-screen-host"
if [ ! -x "$BIN" ]; then
  BIN="$DIR/target/debug/treelink-screen-host"
fi
if [ ! -x "$BIN" ]; then
  echo "Building treelink-screen-host..."
  export PATH="$HOME/.cargo/bin:$PATH"
  (cd "$DIR" && cargo build --release -p treelink-screen --bin treelink-screen-host)
  BIN="$DIR/target/release/treelink-screen-host"
fi
exec "$BIN" --request-permissions "$@"
