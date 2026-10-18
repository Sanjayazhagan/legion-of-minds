# Build from the project root, so the image can see the game's godcore crate:
#   docker build -f backend/Dockerfile -t become-a-god-server .
# (docker compose in this folder does that for you.)
FROM rust:1.92-bookworm AS build
WORKDIR /src
COPY game game
COPY backend backend
WORKDIR /src/backend
RUN cargo build --release --locked -p bag-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 bag
COPY --from=build /src/backend/target/release/bag-server /usr/local/bin/bag-server
USER bag
ENV PORT=8080 RUST_LOG=info,sqlx=warn
EXPOSE 8080
CMD ["bag-server"]
