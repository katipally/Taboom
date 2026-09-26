#!/bin/sh
set -eu

script=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)/install-fortress.sh

TABOOM_ENGINES=chrome TARGETARCH=arm64 sh "$script" --validate-only
TABOOM_ENGINES=chrome,fortress TARGETARCH=amd64 BUILDARCH=amd64 sh "$script" --validate-only

if TABOOM_ENGINES=chrome,fortress TARGETARCH=arm64 BUILDARCH=amd64 sh "$script" --validate-only 2>/dev/null; then
    echo "expected Fortress/arm64 config to fail" >&2
    exit 1
fi
if TABOOM_ENGINES=chrome,fortress TARGETARCH=amd64 BUILDARCH=arm64 sh "$script" --validate-only 2>/dev/null; then
    echo "expected an arm64 builder to fail" >&2
    exit 1
fi
if TABOOM_ENGINES=fortress TARGETARCH=amd64 BUILDARCH=amd64 sh "$script" --validate-only 2>/dev/null; then
    echo "expected unsupported engine config to fail" >&2
    exit 1
fi
if TABOOM_ENGINES=chrome,unknown TARGETARCH=amd64 BUILDARCH=amd64 sh "$script" --validate-only 2>/dev/null; then
    echo "expected invalid engine config to fail" >&2
    exit 1
fi

echo "engine build configuration checks passed"
