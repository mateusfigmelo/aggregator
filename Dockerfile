# Use the official Rust image as a builder
FROM rust:1.82-slim as builder

# Install system dependencies including OpenSSL
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy the entire project
COPY . .

# Build the aggregator binary
RUN cargo build --release --bin aggregator

# Use a minimal runtime image
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    curl \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user
RUN useradd -m -u 1000 aggregator

# Set working directory
WORKDIR /app

# Copy the binary from the builder stage
COPY --from=builder /app/target/release/aggregator /app/aggregator

# Change ownership to the non-root user
RUN chown aggregator:aggregator /app/aggregator

# Switch to non-root user
USER aggregator

# Expose the metrics port
EXPOSE 9000

# Set the entrypoint
ENTRYPOINT ["/app/aggregator"] 