FROM rust:bookworm

RUN apt-get update \
    && apt-get install --no-install-recommends --yes make \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
