#!/usr/bin/env bash
set -euo pipefail

DIR="${1:-.fixture}"
if [ -L "$DIR" ]; then
  echo "refusing symlinked fixture target: $DIR" >&2
  exit 2
fi
if [ -d "$DIR" ] && [ -f "$DIR/.ready" ] && [ ! -L "$DIR/.ready" ] && [ -f "$DIR/.regent-fixture" ] && [ ! -L "$DIR/.regent-fixture" ]; then
  exit 0
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$SCRIPT_DIR/make_fixture.py" "$DIR"
if [ -L "$DIR" ] || [ ! -d "$DIR" ] || [ ! -f "$DIR/.regent-fixture" ] || [ -L "$DIR/.regent-fixture" ]; then
  echo "fixture validation failed for: $DIR" >&2
  exit 2
fi
touch "$DIR/.ready"
