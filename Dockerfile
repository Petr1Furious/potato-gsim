# potato-gsim dedicated server.
#   docker build -t potato-gsim-server .
#   docker run -d --name gsim -p 27777:27777/udp -p 27777:27777/tcp -v gsim-data:/data potato-gsim-server
FROM rust:1-slim-bookworm AS build
RUN rustup target add wasm32-unknown-unknown
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
COPY web ./web
COPY scripts/web.sh ./scripts/web.sh
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p gsim-server -p gsim-client-core \
    && cp target/release/gsim-server target/release/gsim-bot /usr/local/bin/ \
    && scripts/web.sh && cp -r target/web /web

FROM debian:bookworm-slim
RUN useradd --system --no-create-home gsim && mkdir /data && chown gsim /data
COPY --from=build /usr/local/bin/gsim-server /usr/local/bin/gsim-bot /usr/local/bin/
# The client for browsers, handed out on the TCP port.
COPY --from=build /web /web
ENV GSIM_WEB_DIR=/web
USER gsim
# Player names, whitelist and ban lists live here; mount a volume to keep them.
ENV GSIM_STATE_DIR=/data
VOLUME /data
EXPOSE 27777/udp 27777/tcp
ENTRYPOINT ["gsim-server"]
