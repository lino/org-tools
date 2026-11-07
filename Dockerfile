# Copyright (C) 2026 org-tools contributors
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Multi-stage container build for the standalone `org` CLI and MCP server.

ARG RUST_IMAGE=rust:alpine
ARG RUNTIME_IMAGE=alpine:3.21

# region Builder Stage
FROM ${RUST_IMAGE} AS builder

RUN apk add --no-cache build-base musl-dev bash

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY scripts ./scripts
COPY schemas ./schemas

RUN cargo build --release --locked -p org-tools && \
    bash scripts/build-completions.sh --bin target/release/org --output-dir /out/completions
# endregion

# region Runtime Stage
FROM ${RUNTIME_IMAGE} AS runtime

LABEL org.opencontainers.image.title="org-tools" \
      org.opencontainers.image.description="Standalone Org-mode CLI for linting, formatting, querying, agenda, clocking, and MCP without Emacs" \
      org.opencontainers.image.source="https://github.com/lino/org-tools" \
      org.opencontainers.image.licenses="GPL-3.0-or-later"

RUN apk add --no-cache ca-certificates git

COPY --from=builder /src/target/release/org /usr/local/bin/org
COPY --from=builder /out/completions/org.bash /usr/share/bash-completion/completions/org
COPY --from=builder /out/completions/_org /usr/share/zsh/site-functions/_org
COPY --from=builder /out/completions/org.fish /usr/share/fish/vendor_completions.d/org.fish
COPY LICENSE README.org /usr/share/doc/org-tools/

ENV ORG_TOOLS_WORKSPACE=/workspace
WORKDIR /workspace

ENTRYPOINT ["org"]
CMD ["--help"]
# endregion
