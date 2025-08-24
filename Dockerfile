# Multi-stage build for LiNa Store
# Stage 1: Build stage
FROM rust:1.87.0-alpine3.22 as builder

# Install build dependencies
RUN apk add --no-cache \
    pkg-config \
    openssl-dev \
    musl-dev \
    sqlite-dev \
    zlib-dev \
    build-base

# Set working directory
WORKDIR /app

# Copy Cargo files for dependency caching
COPY src/ linabase/ Cargo.toml Cargo.lock .

# Build dependencies only (this layer will be cached if Cargo files don't change)
RUN cargo build --release


# Stage 2: Runtime stage
FROM alpine:3.22

# Install runtime dependencies
RUN apk add --no-cache \
    ca-certificates \
    openssl \
    sqlite

# Create non-root user for security
RUN addgroup -g 1000 linastore && adduser -D -s /bin/sh -u 1000 -G linastore linastore

# Create necessary directories
RUN mkdir -p /app/data /app/logs && \
    chown -R linastore:linastore /app

# Set working directory
WORKDIR /app

# Copy binary from builder stage
COPY --from=builder /app/target/release/linastore-server /usr/local/bin/

# Switch to non-root user
USER linastore

# Expose default ports (adjust based on your application needs)
EXPOSE 8086 8096

# Set environment variables
ENV RUST_LOG=info

# Default command
CMD ["linastore-server"]