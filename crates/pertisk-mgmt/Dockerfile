# Multi-stage: React UI + Rust pertisk-mgmt (single port 8080)

FROM node:22-alpine AS ui
WORKDIR /ui
COPY web/mgmt-ui/package.json web/mgmt-ui/package-lock.json* ./
RUN npm ci || npm install
COPY web/mgmt-ui/ ./
RUN npm run build

FROM rust:1.88-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY proto ./proto
COPY --from=ui /ui/dist ./crates/pertisk-mgmt/static
RUN cargo build --release -p pertisk-mgmt

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/pertisk-mgmt /usr/local/bin/pertisk-mgmt
ENV MGMT_LISTEN=0.0.0.0:8080 \
    MGMT_DB=/data/mgmt.db \
    MGMT_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 8080
ENTRYPOINT ["pertisk-mgmt"]
