FROM ghcr.io/sksat/cargo-chef-docker:1.65.0-bullseye as chef

WORKDIR /build

FROM chef as planner
COPY . .

WORKDIR /build/kble

RUN cargo chef prepare --recipe-path recipe.json

# build
FROM chef as builder
# build deps(cached)
COPY --from=planner /build .

WORKDIR /build/kble

RUN cargo chef cook --recipe-path recipe.json -p kble

# build bin
WORKDIR /build

COPY . .

WORKDIR /build/kble
RUN cargo build

FROM debian:bullseye-slim AS runtime
RUN apt-get update -y && apt-get install -y ca-certificates tini && rm -rf /var/lib/apt/lists/* && update-ca-certificates
WORKDIR /app
COPY --from=builder /build/kble/target/debug/kble /usr/local/bin
ENTRYPOINT [ "/usr/bin/tini", "-e", "143", "--" ]
CMD ["/usr/local/bin/kble"]
