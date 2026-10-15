# Paperrust API image — multi-stage.
# Build context: repo root (run `npm run build` first; libpdfium fetched at build).
#
# Bind: the image sets PAPERRUST_BIND=0.0.0.0:8788. The process accepts that
# only when PAPERRUST_BIND_ALLOW_UNSPECIFIED=1 (PAPERRUST_BIND_K8S=1 is the same gate).

# ---- Rust release ----
FROM docker.io/library/rust:1-bookworm AS build
WORKDIR /src
COPY backend/Cargo.toml backend/Cargo.lock ./backend/
COPY backend/src ./backend/src
# seed.rs: include_str!("../seed_docs.json") — required at compile time
COPY backend/seed_docs.json ./backend/seed_docs.json
WORKDIR /src/backend
RUN cargo build --release --locked

# ---- pdfium ----
FROM docker.io/library/debian:bookworm-slim AS pdfium
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*
# Linux x64 from pdfium-binaries (bblanchon). Pin a release asset URL for reproducible builds.
ARG PDFIUM_URL=https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-linux-x64.tgz
WORKDIR /pdfium
RUN curl -fsSL -o pdfium.tgz "$PDFIUM_URL" \
  && tar -xzf pdfium.tgz \
  && mkdir -p /out \
  && if [ -f lib/libpdfium.so ]; then cp -a lib/libpdfium.so /out/; \
     elif [ -f libpdfium.so ]; then cp -a libpdfium.so /out/; \
     else echo "libpdfium.so missing from archive" >&2; exit 1; fi

# ---- runtime ----
FROM docker.io/library/debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd --system --gid 10001 paperrust \
  && useradd --system --uid 10001 --gid paperrust --home-dir /app --shell /usr/sbin/nologin paperrust
WORKDIR /app
COPY --from=build /src/backend/target/release/paperrust-api /app/paperrust-api
COPY --from=pdfium /out/libpdfium.so /app/lib/libpdfium.so
COPY dist/ /app/dist/
ENV PAPERRUST_UI_DIR=/app/dist \
    PAPERRUST_PDFIUM=/app/lib/libpdfium.so \
    PAPERRUST_ENV=production \
    PAPERRUST_BIND=0.0.0.0:8788
# 0.0.0.0 is refused until PAPERRUST_BIND_ALLOW_UNSPECIFIED=1 or PAPERRUST_BIND_K8S=1.
USER 10001:10001
EXPOSE 8788
# Prefer K8s probes GET /health over Docker HEALTHCHECK.
ENTRYPOINT ["/app/paperrust-api"]
