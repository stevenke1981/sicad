# syntax=docker/dockerfile:1

# --- Stage 1: build the React frontend (web/) -------------------------------------------
# vite.config.ts writes the bundle to ../app/web_dist, i.e. /src/app/web_dist here.
# The generated API types (web/src/api/generated) are committed, so gen:types is not needed.
FROM node:24-slim AS web
WORKDIR /src/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

# --- Stage 2: build the Rust core (crates/promptcad-core, ADR 0005/0012) ---------------
# Same Debian release (trixie, glibc 2.41) as the micromamba runtime image below, so the
# dynamically linked binary runs there unchanged; no musl target / extra toolchain needed.
# rust-version in Cargo.toml is 1.90, so 1.90.0 is the oldest toolchain that builds it
# (verified). The repo-root rust-toolchain.toml (a Windows MSVC pin) is not copied here.
FROM rust:1.90.0-slim-trixie AS core
WORKDIR /src/crates/promptcad-core
COPY crates/promptcad-core/Cargo.toml crates/promptcad-core/Cargo.lock ./
COPY crates/promptcad-core/src ./src
RUN cargo build --release --locked --bins

# --- Stage 3: CadQuery runtime (conda-forge via micromamba) + FastAPI --------------------
# Pinned instead of :latest so a micromamba release cannot change the build underneath us;
# 2.10.0 is the Debian 13 slim image that :latest pointed to when this was written.
FROM mambaorg/micromamba:2.10.0

ARG MAMBA_DOCKERFILE_ACTIVATE=1

# WORKDIR alone may create /app as root; the editable install below writes egg-info there.
USER root
RUN mkdir -p /app && chown "$MAMBA_USER:$MAMBA_USER" /app
USER $MAMBA_USER
WORKDIR /app

COPY --chown=$MAMBA_USER:$MAMBA_USER environment.runtime.yml /tmp/environment.yml
RUN micromamba install --yes --name base --file /tmp/environment.yml \
    && micromamba clean --all --yes

COPY --chown=$MAMBA_USER:$MAMBA_USER . /app
RUN python -m pip install --no-build-isolation --no-deps --editable .

# Served at "/" by app.main (Settings.frontend_dir defaults to app/web_dist). Without it
# "/" would only return the 503 "frontend not built" page.
COPY --from=web --chown=$MAMBA_USER:$MAMBA_USER /src/app/web_dist /app/app/web_dist

# The server runs `promptcad-core rpc` once per POST /api/v1/core call (ADR 0012).
COPY --from=core --chmod=0755 /src/crates/promptcad-core/target/release/promptcad-core /usr/local/bin/promptcad-core
ENV PROMPTCAD_CORE_BIN=/usr/local/bin/promptcad-core

EXPOSE 8000
VOLUME ["/app/generated"]
CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
