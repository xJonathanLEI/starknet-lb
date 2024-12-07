FROM rust:1.83-alpine AS build

RUN apk add --update alpine-sdk

WORKDIR /src
COPY . /src

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release

RUN --mount=type=cache,target=/src/target \
    mv ./target/release/starknet-lb .

FROM alpine:latest

LABEL org.opencontainers.image.source=https://github.com/xJonathanLEI/starknet-lb

COPY --from=build /src/starknet-lb /usr/bin/

ENTRYPOINT [ "starknet-lb" ]
