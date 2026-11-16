# syntax=docker/dockerfile:1
FROM --platform=$BUILDPLATFORM rust:1.96.0-bookworm AS build
RUN --mount=type=secret,id=build_ca \
    apt-get update && apt-get install -y --no-install-recommends ca-certificates python3 \
    && rm -rf /var/lib/apt/lists/* \
    && if [ -f /run/secrets/build_ca ]; then \
           install -m 0644 /run/secrets/build_ca /usr/local/share/ca-certificates/build-network.crt \
           && update-ca-certificates; \
       fi \
    && rustup target add wasm32-unknown-unknown \
    && cargo install --locked wasm-bindgen-cli --version 0.2.130
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml build.rs LICENSE NOTICE ./
COPY build ./build
COPY src ./src
COPY rules ./rules
COPY scripts/build-web.py ./scripts/build-web.py
COPY web ./web
RUN python3 scripts/build-web.py

FROM nginx:stable-alpine
LABEL org.opencontainers.image.title="automaton-worm" \
      org.opencontainers.image.description="An Autonomous CA worm simulator" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /app/dist/ /usr/share/nginx/html/
EXPOSE 80
