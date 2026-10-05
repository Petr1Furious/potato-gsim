# potato-gsim dedicated server.
#   docker build -t potato-gsim-server .
#   docker run -d --name gsim -p 27777:27777/udp -v gsim-data:/data potato-gsim-server
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p gsim-server -p gsim-client-core \
    && cp target/release/gsim-server target/release/gsim-bot /usr/local/bin/

FROM debian:bookworm-slim
RUN useradd --system --no-create-home gsim && mkdir /data && chown gsim /data
COPY --from=build /usr/local/bin/gsim-server /usr/local/bin/gsim-bot /usr/local/bin/
USER gsim
# Player names, whitelist and ban lists live here; mount a volume to keep them.
ENV GSIM_STATE_DIR=/data
VOLUME /data
EXPOSE 27777/udp
ENTRYPOINT ["gsim-server"]
