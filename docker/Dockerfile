# Runtime image for orchestratord. Built from artifacts that scripts/ci.sh produced inside
# the builder image (dist/orchestratord, strand-agent/dist/strand-agent.wasm).
FROM debian:bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 --home /var/lib/taffee taffee \
 && mkdir -p /opt/taffee/agents /var/cache/taffee /var/lib/taffee \
 && chown taffee /var/cache/taffee /var/lib/taffee

COPY dist/orchestratord /usr/local/bin/orchestratord
COPY strand-agent/dist/strand-agent.wasm /opt/taffee/agents/strand-agent.wasm
# Intent kinds for the work manifest API. /v1/work is served when TAFFEE_ORCH_KINDS_DIR is set
# (or the config file has a `work` section).
COPY docs/spec/work-manifest/kinds/ /opt/taffee/kinds/
# The example repository the work manifest spec uses, so a fresh service can do real work.
COPY docs/spec/work-manifest/fixtures/ /opt/taffee/inputs/

# What this image is: the source commit (with -dirty if it had uncommitted changes).
ARG TAFFEE_VERSION=dev
LABEL org.opencontainers.image.version=$TAFFEE_VERSION

USER taffee
ENV TAFFEE_ORCH_VERSION=$TAFFEE_VERSION \
    TAFFEE_ORCH_LISTEN=0.0.0.0:8780 \
    TAFFEE_ORCH_COMPONENT=/opt/taffee/agents/strand-agent.wasm \
    TAFFEE_ORCH_CACHE_DIR=/var/cache/taffee \
    RUST_LOG=info
EXPOSE 8780
HEALTHCHECK --interval=10s --timeout=3s --start-period=90s --retries=6 \
  CMD bash -c 'exec 3<>/dev/tcp/127.0.0.1/8780 && printf "GET /healthz HTTP/1.0\r\n\r\n" >&3 && grep -q "\"ok\":true" <&3'
ENTRYPOINT ["/usr/local/bin/orchestratord"]
