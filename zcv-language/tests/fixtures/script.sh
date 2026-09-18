#!/usr/bin/env bash
set -euo pipefail

readonly MAX_RETRIES=3

greet() {
    local name="${1:-Zcv}"
    printf 'Hello, %s!\n' "$name"
}

for attempt in $(seq 1 "$MAX_RETRIES"); do
    if [[ "$attempt" -eq 1 ]]; then
        greet "Shell"
    fi
done
