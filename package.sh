#!/bin/bash

# package.sh - Build and package LiNaStore for all supported platforms
# Usage: ./package.sh [version]

set -e  # Exit on any error

VERSION=${1:-"unknown"}

# Define variables
PROJECT_NAME="LiNaStore"
BUILD_DIR="build"


# Function to check if Rust is installed
check_rust() {
    if command -v rustc >/dev/null 2>&1; then
        RUST_VERSION=$(rustc --version)
        echo "Rust is already installed: $RUST_VERSION"
        return 0
    else
        echo "Rust is not installed"
        return 1
    fi
}

# Function to install Rust
install_rust() {
    echo "Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    
    # Source cargo environment
    source "$HOME/.cargo/env"
    
    if command -v rustc >/dev/null 2>&1; then
        RUST_VERSION=$(rustc --version)
        echo "Rust successfully installed: $RUST_VERSION"
    else
        echo "Failed to install Rust"
        exit 1
    fi
}

# Function to check if mingw-w64 is installed (needed for Windows cross-compilation)
check_mingw_w64() {
    if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
        echo "mingw-w64 is already installed"
        return 0
    else
        echo "mingw-w64 is not installed"
        return 1
    fi
}

# Function to install mingw-w64
install_mingw_w64() {
    echo "Installing mingw-w64..."
    
    # Detect OS
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        # Install mingw-w64 for Ubuntu/Debian
        sudo apt-get update
        sudo apt-get install -y mingw-w64
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        # On macOS, use Homebrew
        if ! command -v brew >/dev/null 2>&1; then
            echo "Homebrew is not installed. Please install Homebrew first: https://brew.sh/"
            exit 1
        fi
        brew install mingw-w64
    else
        echo "Unsupported OS. Please install mingw-w64 manually."
        exit 1
    fi
    
    # Verify installation
    if command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
        echo "mingw-w64 successfully installed"
    else
        echo "Failed to install mingw-w64"
        exit 1
    fi
}

# Check and install Rust if needed
echo "Checking Rust installation..."
if ! check_rust; then
    install_rust
    # Source cargo environment for current session
    source "$HOME/.cargo/env"
fi

# Check and install mingw-w64 if needed (for Windows cross-compilation)
echo "Checking mingw-w64 installation (needed for Windows cross-compilation)..."
if ! check_mingw_w64; then
    install_mingw_w64
fi

# Create build directory
echo "Creating build directory..."
mkdir -p "${BUILD_DIR}"

# Function to package for a specific platform
package_for_platform() {
    local target=$1
    local platform_name=$2
    
    echo "Packaging for ${platform_name} (${target})..."
    
    # Add target for cross-compilation
    rustup target add "${target}" 2>/dev/null || true
    
    # Build server component (main.rs)
    echo "Building server component..."
    cargo build --release --target "${target}"
    
    # Build binary component (linastore.rs)
    echo "Building binary component..."
    cargo build --release --target "${target}" --bin linastore
    
    # Determine file extensions
    local ext=""
    if [[ "${platform_name}" == *"windows"* ]]; then
        ext=".exe"
    fi
    
    # Copy server binary
    local server_output="${PROJECT_NAME}-server-${platform_name}${ext}"
    cp "target/${target}/release/linastore-server${ext}" "${BUILD_DIR}/${server_output}" 2>/dev/null || \
    cp "target/${target}/release/main${ext}" "${BUILD_DIR}/${server_output}" 2>/dev/null
    
    # Copy binary component
    local bin_output="${PROJECT_NAME}-bin-${platform_name}${ext}"
    cp "target/${target}/release/linastore${ext}" "${BUILD_DIR}/${bin_output}" 2>/dev/null
    
    echo "Created ${BUILD_DIR}/${server_output}"
    echo "Created ${BUILD_DIR}/${bin_output}"
}

# Package for all platforms
echo "Starting packaging process for version: ${VERSION}"

# Linux packaging
package_for_platform "x86_64-unknown-linux-musl" "linux-x86_64"

# Windows packaging
package_for_platform "x86_64-pc-windows-gnu" "windows-x86_64"

echo "All packages created successfully in ${BUILD_DIR}/"