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

WORKDIR /app

# Copy Cargo files for dependency caching (layer optimization)
COPY Cargo.toml Cargo.lock ./
COPY linabase/Cargo.toml ./linabase/
COPY linastore-server/Cargo.toml ./linastore-server/

# Create dummy source files to build dependencies only
RUN mkdir -p linabase/src linastore-server/src \
    && echo "" > linabase/src/lib.rs \
    && echo "fn main() {}" > linastore-server/src/main.rs \
    # Build dependencies (this layer is cached if Cargo files don't change)
    && cargo build --release -p linastore-server \
    # Remove dummy source files
    && rm -rf linabase/src linastore-server/src

# Copy actual source code
COPY linastore-server/src/ ./linastore-server/src/
COPY linabase/src/ ./linabase/src/

# Ensure new source is picked up
RUN touch linastore-server/src/main.rs linabase/src/lib.rs

# Build the actual binary (dependencies already compiled)
RUN cargo build --release -p linastore-server


# Stage 2: Runtime stage
FROM alpine:3.22

# Install runtime dependencies
RUN apk add --no-cache \
    ca-certificates \
    openssl \
    sqlite-libs

# Create non-root user
RUN addgroup -S linastore && adduser -S linastore -G linastore

WORKDIR /app

# Copy binary from builder stage
COPY --from=builder /app/target/release/linastore-server /usr/local/bin/

# Switch to non-root user
USER linastore

# Expose default ports
EXPOSE 8086 8096

# Set environment variables
ENV RUST_LOG=info

# Default command
CMD ["linastore-server", "start", "--foreground"]
