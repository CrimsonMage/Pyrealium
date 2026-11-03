# Build from the repository root: docker compose -f deploy/docker/compose.yml up --build
FROM rust:1.90-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends python3 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN python3 export_source.py \
 && cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --home /var/lib/pyrealium pyrealium && mkdir -p /var/lib/pyrealium/run \
 && chown -R pyrealium /var/lib/pyrealium
COPY --from=build /src/target/release/pyrealium /opt/pyrealium/pyrealium
COPY --from=build /src/content /opt/pyrealium/content
COPY deploy/docker/pyrealium.toml /etc/pyrealium/pyrealium.toml
USER pyrealium
WORKDIR /var/lib/pyrealium
EXPOSE 9000/udp 9001/udp 19400/tcp
ENTRYPOINT ["/opt/pyrealium/pyrealium", "run", "--config", "/etc/pyrealium/pyrealium.toml"]
