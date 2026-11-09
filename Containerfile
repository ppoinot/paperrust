# Paperrust API image — multi-stage.
# Build context: repo root. The ui stage builds dist/ from package-lock.json
# (npm ci). libpdfium is fetched at build. Do not COPY a host dist/.
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
# IPv4, six attempts, connect timeout 15s, max time 40s.
ARG PDFIUM_URL=https://github.com/bblanchon/pdfium-binaries/releases/latest/download/pdfium-linux-x64.tgz
WORKDIR /pdfium
RUN set -eu; \
    i=1; \
    while [ "$i" -le 6 ]; do \
      rm -f pdfium.tgz; \
      if curl -4 -fL --connect-timeout 15 --max-time 40 -o pdfium.tgz "$PDFIUM_URL"; then \
        break; \
      fi; \
      echo "pdfium download attempt ${i} failed" >&2; \
      if [ "$i" -eq 6 ]; then exit 1; fi; \
      i=$((i + 1)); \
      sleep $((i * 2)); \
    done; \
    tar -xzf pdfium.tgz \
    && mkdir -p /out \
    && if [ -f lib/libpdfium.so ]; then cp -a lib/libpdfium.so /out/; \
       elif [ -f libpdfium.so ]; then cp -a libpdfium.so /out/; \
       else echo "libpdfium.so missing from archive" >&2; exit 1; fi

# ---- frontend (dist/) ----
FROM docker.io/library/node:22.23.3-bookworm-slim@sha256:c3de60bf2f9dd0ac6370e6117950ff62d6e339527e7472301c9c78a017978392 AS ui
ENV PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 \
    NPM_CONFIG_AUDIT=false \
    NPM_CONFIG_FUND=false
WORKDIR /ui
COPY package.json package-lock.json ./
RUN npm ci --ignore-scripts
COPY . .
RUN rm -rf dist .vercel \
  && node scripts/with-app-env.mjs npx --no-install vite build \
  && node scripts/copy-ui-dist.mjs \
  && cp -a public/. dist/ \
  && test -f dist/index.html && test -f dist/_shell.html && test -d dist/assets \
  && test -f dist/favicon.svg && test -d dist/pdfjs/cmaps && test -d dist/pdfjs/standard_fonts

# Fail the build if dist/ carries secret-looking strings. Prints file names and counts only.
RUN cd /ui/dist \
  && HITS=$(grep -rlIE 'glpat-|xai-[A-Za-z0-9]{20,}|client_secret|-----BEGIN [A-Z ]*PRIVATE KEY|sk-[A-Za-z0-9]{20,}' . | tee /dev/stderr | wc -l) \
  && LOOSE=$(grep -rlIE 'sk-|BEGIN' . | wc -l) \
  && echo "SECRET_SCAN_LOOSE_FILES=$LOOSE (info only: sk-|BEGIN)" \
  && echo "SECRET_SCAN_HITS=$HITS" \
  && test "$HITS" -eq 0

# ---- runtime ----
FROM docker.io/library/debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libstdc++6 \
  && rm -rf /var/lib/apt/lists/* \
  && groupadd --system --gid 10001 paperrust \
  && useradd --system --uid 10001 --gid paperrust --home-dir /app --shell /usr/sbin/nologin paperrust
WORKDIR /app
COPY --from=build /src/backend/target/release/paperrust-api /app/paperrust-api
COPY --from=pdfium /out/libpdfium.so /app/lib/libpdfium.so
COPY --from=ui /ui/dist/ /app/dist/
ENV PAPERRUST_UI_DIR=/app/dist \
    PAPERRUST_PDFIUM=/app/lib/libpdfium.so \
    PAPERRUST_ENV=production \
    PAPERRUST_BIND=0.0.0.0:8788
# 0.0.0.0 is refused until PAPERRUST_BIND_ALLOW_UNSPECIFIED=1 or PAPERRUST_BIND_K8S=1.
USER 10001:10001
EXPOSE 8788
# Prefer K8s probes GET /health over Docker HEALTHCHECK.
ENTRYPOINT ["/app/paperrust-api"]
