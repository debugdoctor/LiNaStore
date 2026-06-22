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

# Create dummy source files to build dependencies only
RUN mkdir -p src linabase/src \
    && echo "fn main() {}" > src/main.rs \
    && echo "" > linabase/src/lib.rs \
    # Build dependencies (this layer is cached if Cargo files don't change)
    && cargo build --release \
    # Remove dummy source files
    && rm -rf src linabase/src

# Copy actual source code
COPY src/ ./src/
COPY linabase/src/ ./linabase/src/

# Ensure new source is picked up
RUN touch src/main.rs linabase/src/lib.rs

# Build the actual binary (dependencies already compiled)
RUN cargo build --release


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
COPY --from=builder /app/target/release/linastore /usr/local/bin/

# Switch to non-root user
USER linastore

# Expose default ports
EXPOSE 8086 8096

# Set environment variables
ENV RUST_LOG=info

# Default command
CMD ["linastore", "start", "--foreground"]
