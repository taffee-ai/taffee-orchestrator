# Builder image for taffee-orchestrator: Rust (host), Node (guest component), Python (mocks).
# Every build and test runs inside this image (see AGENTS.md). Nothing is installed on a host.
FROM rust:1.96-bookworm

ARG NODE_VERSION=20.20.2
RUN apt-get update \
 && apt-get install -y --no-install-recommends python3 curl ca-certificates xz-utils \
 && rm -rf /var/lib/apt/lists/* \
 && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
    | tar -xJ -C /usr/local --strip-components=1 \
 && node --version && npm --version

# The exact toolchain runner/rust-toolchain.toml pins, so rustup never downloads at run time.
RUN rustup toolchain install 1.96.0 --profile minimal && rustup default 1.96.0

ENV CARGO_TARGET_DIR=/work/target \
    CARGO_TERM_COLOR=never
WORKDIR /src
