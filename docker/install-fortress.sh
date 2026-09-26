#!/bin/sh
set -eu

FORTRESS_VERSION=150.0.7871.114
FORTRESS_ASSET=tilion-fortress-linux-x64.tar.gz
ENGINES=${TABOOM_ENGINES-chrome}
TARGET_ARCH=${TARGETARCH-unknown}
BUILD_ARCH=${BUILDARCH-unknown}

case "$ENGINES" in
    chrome)
        ;;
    chrome,fortress)
        if [ "$TARGET_ARCH" != amd64 ]; then
            echo "Fortress supports only Linux amd64; requested Docker target architecture is '$TARGET_ARCH'" >&2
            exit 1
        fi
        if [ "$BUILD_ARCH" != amd64 ]; then
            echo "Fortress builds require an amd64 BuildKit builder; detected builder architecture '$BUILD_ARCH'" >&2
            exit 1
        fi
        ;;
    *)
        echo "invalid TABOOM_ENGINES='$ENGINES'; supported values are 'chrome' and 'chrome,fortress'" >&2
        exit 1
        ;;
esac

if [ "${1-}" = --validate-only ]; then
    exit 0
fi

mkdir -p /usr/local/share/taboom
if [ "$ENGINES" = chrome ]; then
    printf 'chrome\n' > /usr/local/share/taboom/engines
    exit 0
fi

printf 'chrome\nfortress\n' > /usr/local/share/taboom/engines
base="https://github.com/tiliondev/fortress/releases/download/v$FORTRESS_VERSION"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM

curl --fail --location --retry 3 --silent --show-error \
    --output "$work/$FORTRESS_ASSET" "$base/$FORTRESS_ASSET"
curl --fail --location --retry 3 --silent --show-error \
    --output "$work/SHA256SUMS" "$base/SHA256SUMS"

# Require exactly one entry for the archive we downloaded; --ignore-missing by itself could
# silently accept a manifest that no longer covers this asset.
record=$(awk -v asset="$FORTRESS_ASSET" \
    '$2 == asset || $2 == "*" asset { print $1 "  " asset; count++ }
     END { if (count != 1) exit 1 }' "$work/SHA256SUMS") || {
    echo "official Fortress v$FORTRESS_VERSION SHA256SUMS has no unique $FORTRESS_ASSET entry" >&2
    exit 1
}
(cd "$work" && printf '%s\n' "$record" | sha256sum --check --status) || {
    echo "Fortress v$FORTRESS_VERSION failed its official SHA256SUMS check" >&2
    exit 1
}

mkdir -p /opt/fortress
tar --no-same-owner --extract --gzip --file "$work/$FORTRESS_ASSET" \
    --directory /opt/fortress --strip-components=1
if [ ! -x /opt/fortress/chrome ]; then
    echo "Fortress v$FORTRESS_VERSION bundle did not contain executable /opt/fortress/chrome" >&2
    exit 1
fi
if [ ! -x /opt/fortress/tilion ]; then
    echo "Fortress v$FORTRESS_VERSION bundle did not contain its /opt/fortress/tilion launcher" >&2
    exit 1
fi

# Keep the upstream launcher, which supplies Fortress's bundled-font and graphics setup, while
# letting Taboom's locale-specific fontconfig remain authoritative.
sed -i 's|^export FONTCONFIG_FILE="$CONF"$|export FONTCONFIG_FILE="${TABOOM_FONTCONFIG_FILE:-$CONF}"|' /opt/fortress/tilion
if ! grep -Fq 'export FONTCONFIG_FILE="${TABOOM_FONTCONFIG_FILE:-$CONF}"' /opt/fortress/tilion; then
    echo "could not configure the Fortress launcher to preserve Taboom's fontconfig" >&2
    exit 1
fi
