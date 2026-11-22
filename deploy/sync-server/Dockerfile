FROM rust:1-slim-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY .cargo/ .cargo/
COPY crates/ crates/
RUN cargo build --release --locked -p atoll-sync-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home atoll
COPY --from=build /build/target/release/atoll-sync-server /usr/local/bin/atoll-sync-server
USER atoll
ENV ATOLL_SYNC_LISTEN=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s CMD ["atoll-sync-server", "health"]
ENTRYPOINT ["atoll-sync-server"]
