# Multi-stage build for LiNa Store
# Stage 1: Build stage
FROM rust:1.87.0-alpine3.22 AS builder

# Install build dependencies
RUN apk add --no-cache \
    pkgconfig \
    openssl-dev \
    musl-dev \
    sqlite-dev \
    zlib-dev \
    build-base

# Set working directory
WORKDIR /app

# Copy Cargo files for dependency caching
COPY . .

# Build dependencies only (this layer will be cached if Cargo files don't change)
RUN cargo build --release


# Stage 2: Runtime stage
FROM alpine:3.22

# Install runtime dependencies
RUN apk add --no-cache \
    ca-certificates \
    openssl

# Set working directory
WORKDIR /app

# Copy binary from builder stage
COPY --from=builder /app/target/release/linastore-server /usr/local/bin/

# Switch to non-root user
USER root

# Expose default ports (adjust based on your application needs)
EXPOSE 8086 8096

# Set environment variables
ENV RUST_LOG=info

# Default command
CMD ["linastore-server"]