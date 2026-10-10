# Build environment for the AppImage (scripts/build-appimage.sh). Ubuntu 22.04 has glibc 2.35, so the binary runs on that and every newer distribution.
FROM ubuntu:22.04

ARG GO_VERSION=1.26.1
ARG APPIMAGETOOL_VERSION=1.9.1

RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends build-essential pkg-config libdbus-1-dev libclang-dev clang curl ca-certificates git file desktop-file-utils \
 && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup PATH=/opt/cargo/bin:/opt/go/bin:$PATH
RUN curl -sSf https://sh.rustup.rs | CARGO_HOME=/opt/cargo sh -s -- -y --profile minimal --default-toolchain stable --no-modify-path \
 && chmod -R a+rX /opt/rustup /opt/cargo
RUN curl -sSfL https://go.dev/dl/go${GO_VERSION}.linux-amd64.tar.gz | tar -xz -C /opt
RUN curl -sSfLo /opt/appimagetool https://github.com/AppImage/appimagetool/releases/download/${APPIMAGETOOL_VERSION}/appimagetool-x86_64.AppImage \
 && chmod a+x /opt/appimagetool

COPY build-inside.sh /opt/build-inside.sh
