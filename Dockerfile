FROM rust:1.94 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release --bin peat-gateway --features full

FROM cgr.dev/chainguard/glibc-dynamic:latest AS chainguard
COPY --from=builder /build/target/release/peat-gateway /usr/local/bin/
EXPOSE 8080 11204 11205
ENTRYPOINT ["peat-gateway"]

FROM registry.access.redhat.com/ubi9/ubi-minimal:latest AS ubi
RUN microdnf install -y shadow-utils && microdnf clean all && \
    groupadd -r peat && useradd -r -g peat -d /var/lib/peat-gateway -s /sbin/nologin peat && \
    mkdir -p /var/lib/peat-gateway && chown peat:peat /var/lib/peat-gateway
COPY --from=builder /build/target/release/peat-gateway /usr/local/bin/
EXPOSE 8080 11204 11205
USER peat
ENV PEAT_GATEWAY_DATA_DIR=/var/lib/peat-gateway
ENTRYPOINT ["peat-gateway"]
