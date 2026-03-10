FROM rust:1.94 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin peat-gateway --features full

FROM cgr.dev/chainguard/glibc-dynamic:latest
COPY --from=builder /build/target/release/peat-gateway /usr/local/bin/
EXPOSE 8080 11204 11205
ENTRYPOINT ["peat-gateway"]
