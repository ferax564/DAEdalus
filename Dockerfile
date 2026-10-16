# Builds the DAEdalus web workbench (Rust -> WebAssembly + Vite) and serves it
# as static files with nginx on port 80.

# 1. Compile the solver to WebAssembly and generate the JS bindings.
FROM rust:1.97-slim-bookworm AS wasm
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
# wasm-bindgen-cli must match the wasm-bindgen crate version (=0.2.129).
ARG WASM_BINDGEN_VERSION=0.2.129
RUN curl -fsSL "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    | tar -xz -C /tmp \
    && mv /tmp/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen /usr/local/bin/
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY crates ./crates
# The library index is compiled into daedalus-core (`libraries::INDEX_JSON`).
COPY library/packages.json ./library/packages.json
ENV CARGO_NET_GIT_FETCH_WITH_CLI=true
RUN cargo build -p daedalus-wasm --profile release-wasm --target wasm32-unknown-unknown \
    && wasm-bindgen --keep-lld-exports --target web --out-dir /out \
       target/wasm32-unknown-unknown/release-wasm/daedalus_wasm.wasm

# 2. Bundle the workbench.
FROM node:22-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
# The Modelica Standard Library 4.1.0, downloaded from GitHub and packed into
# public/msl/ (its own layer, so code changes do not download it again).
COPY web/scripts ./scripts
RUN node scripts/fetch-msl.mjs
COPY web ./
COPY library /src/library
# The documentation site in docs/ is rendered to dist/docs/ and served at /docs/;
# docs/conformance feeds the scoreboard page (scripts/copy-conformance.mjs).
COPY docs /src/docs
COPY --from=wasm /out ./src/wasm
# Binaryen's optimizer: a faster and about 20% smaller module than rustc alone.
RUN npm run wasm-opt
RUN npm run build

# 3. Serve the static bundle.
FROM nginx:1.27-alpine
COPY deploy/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web /src/web/dist /usr/share/nginx/html
EXPOSE 80
