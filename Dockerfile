FROM rust:1-alpine3.23 AS builder
ENV RUSTFLAGS="-C target-feature=-crt-static"
RUN apk add --no-cache musl-dev \
    # Required for git-version
    git

WORKDIR /pumpkinpie
COPY . /pumpkinpie

RUN rustup show active-toolchain || rustup toolchain install
RUN rustup component add rustfmt

# build release
RUN --mount=type=cache,sharing=private,target=/pumpkinpie/target \
    --mount=type=cache,target=/usr/local/cargo/git/db \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    cargo build --release -p pumpkinpie && cp target/release/pumpkinpie ./pumpkinpie.release

FROM alpine:3.24

COPY --from=builder /pumpkinpie/pumpkinpie.release /bin/pumpkinpie

# Keep server data separate from the PumpkinPie executable.
# This allows bind mounting the server files without overwriting the PumpkinPie
# executable (without requiring an `docker cp`-ing the binary to the host folder)
WORKDIR /pumpkinpie

RUN apk add --no-cache libgcc && chown 2613:2613 .

ENV RUST_BACKTRACE=1
EXPOSE 25565
USER 2613:2613
ENTRYPOINT [ "/bin/pumpkinpie" ]
HEALTHCHECK CMD nc -z 127.0.0.1 25565 || exit 1
