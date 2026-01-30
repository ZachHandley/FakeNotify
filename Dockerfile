# Multi-stage build for FakeNotify
FROM rust:1.84-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /build
COPY . .

RUN cargo build --release

# Runtime image - just the daemon and library
FROM alpine:3.21

COPY --from=builder /build/target/release/fakenotifyd /usr/local/bin/
COPY --from=builder /build/target/release/libfakenotify_preload.so /usr/local/lib/

# Create socket directory
RUN mkdir -p /run/fakenotify

ENTRYPOINT ["/usr/local/bin/fakenotifyd"]
CMD ["start", "--socket", "/run/fakenotify/fakenotify.sock"]
