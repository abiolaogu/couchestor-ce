# =============================================================================
# Stratum - Minimal Runtime Dockerfile
# =============================================================================
# Build: docker build -t abiolaogu/stratum:latest .
#
# Note: Requires pre-built Linux binary at target/x86_64-unknown-linux-gnu/release/
# Build binary first: cargo zigbuild --release --target x86_64-unknown-linux-gnu
# =============================================================================

FROM gcr.io/distroless/static-debian12:nonroot

# Labels
LABEL org.opencontainers.image.title="Stratum"
LABEL org.opencontainers.image.description="Intelligent tiered storage operator with erasure coding"
LABEL org.opencontainers.image.vendor="Abiola Ogunsakin"
LABEL org.opencontainers.image.source="https://github.com/abiolaogu/OpenEBS-Mayastor-Fork"
LABEL org.opencontainers.image.licenses="Apache-2.0"
LABEL org.opencontainers.image.version="1.0.0"

WORKDIR /

# Copy pre-built Linux binary
COPY target/x86_64-unknown-linux-gnu/release/stratum /stratum

# Run as non-root
USER nonroot:nonroot

# Expose ports (metrics: 8080, health: 8081)
EXPOSE 8080 8081

# Set entrypoint
ENTRYPOINT ["/stratum"]
