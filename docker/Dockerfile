# Stage 1: build Rust binaries
FROM rust:1-bookworm AS builder

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

RUN cargo build --release --bin taboomd --bin taboom

# Stage 2: build vinput
FROM debian:bookworm-slim AS input-builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    gcc libc6-dev libwayland-dev wayland-protocols libxkbcommon-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY docker/vinput.c docker/wlr-virtual-pointer-unstable-v1.xml \
     docker/virtual-keyboard-unstable-v1.xml ./

RUN wayland-scanner client-header wlr-virtual-pointer-unstable-v1.xml vpointer-client.h \
    && wayland-scanner private-code wlr-virtual-pointer-unstable-v1.xml vpointer-glue.c \
    && wayland-scanner client-header virtual-keyboard-unstable-v1.xml vkeyboard-client.h \
    && wayland-scanner private-code virtual-keyboard-unstable-v1.xml vkeyboard-glue.c \
    && gcc -O2 -Wall -o vinput vinput.c vpointer-glue.c vkeyboard-glue.c \
        $(pkg-config --cflags --libs wayland-client xkbcommon) \
    && strip vinput

# Stage 3: runtime
FROM debian:bookworm-slim

# Google Chrome from Google's signed apt repo (amd64 and arm64)
ADD --chmod=644 https://dl.google.com/linux/linux_signing_key.pub /etc/apt/keyrings/google-chrome.asc
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/google-chrome.asc] https://dl.google.com/linux/chrome/deb/ stable main" \
        > /etc/apt/sources.list.d/google-chrome.list \
    && rm -rf /var/lib/apt/lists/*

RUN apt-get update && apt-get install -y --no-install-recommends \
    sway grim \
    wayvnc novnc websockify \
    wl-clipboard \
    foot fuzzel \
    wf-recorder ffmpeg \
    google-chrome-stable \
    adwaita-icon-theme \
    fonts-liberation fonts-noto-core \
    dbus-x11 \
    procps \
    && rm -rf /var/lib/apt/lists/*

# taboom user
RUN useradd -m -u 1000 -s /bin/bash taboom \
    && mkdir -p /run/user/1000 \
    && chown taboom:taboom /run/user/1000 \
    && mkdir -p /home/taboom/.config/sway \
       /home/taboom/.taboom/personas \
       /home/taboom/.taboom/profiles \
       /home/taboom/.taboom/run \
       /home/taboom/.taboom/audit \
       /home/taboom/.taboom/logs \
       /home/taboom/.taboom/vault \
       /home/taboom/Downloads \
    && chown -R taboom:taboom /home/taboom

COPY --from=builder /src/target/release/taboomd /usr/local/bin/taboomd
COPY --from=builder /src/target/release/taboom /usr/local/bin/taboom
COPY --from=input-builder /build/vinput /usr/local/bin/vinput
COPY docker/entrypoint.sh /opt/taboom/entrypoint.sh
COPY docker/sway.conf /opt/taboom/sway.conf
COPY docker/taboom-browser docker/taboom-bar /usr/local/bin/
COPY image/chrome-policy.json /etc/opt/chrome/policies/managed/taboom.json
# the 6080 port link (e.g. in Docker Desktop) opens the live view instead of a file listing
COPY docker/novnc-index.html /usr/share/novnc/index.html

RUN chmod +x /opt/taboom/entrypoint.sh \
    /usr/local/bin/taboom-browser /usr/local/bin/taboom-bar

USER taboom
WORKDIR /home/taboom

ENV XDG_RUNTIME_DIR=/run/user/1000
ENV WAYLAND_DISPLAY=wayland-1
ENV TABOOM_HOME=/home/taboom/.taboom

EXPOSE 3456 6080

ENTRYPOINT ["/opt/taboom/entrypoint.sh"]
