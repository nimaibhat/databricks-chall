# Shopping Sessions: native Rust pipeline, static binary on an empty image.
FROM rust:1.91-bookworm AS build
WORKDIR /src
ARG TARGETARCH
ARG XCPU=x86-64-v3
# Scorers are Intel Xeon (Cascade Lake or newer): AVX2 + BMI2 are safe.
ENV CARGO_PROFILE_RELEASE_LTO=fat
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    if [ "$TARGETARCH" = "arm64" ]; then T=aarch64-unknown-linux-gnu; F="-C target-cpu=neoverse-n1"; else T=x86_64-unknown-linux-gnu; F="-C target-cpu=$XCPU"; fi && \
    RUSTFLAGS="$F -C target-feature=+crt-static" cargo build --release --target $T && rm -rf src
COPY src ./src
RUN touch src/main.rs && \
    if [ "$TARGETARCH" = "arm64" ]; then T=aarch64-unknown-linux-gnu; F="-C target-cpu=neoverse-n1"; else T=x86_64-unknown-linux-gnu; F="-C target-cpu=$XCPU"; fi && \
    RUSTFLAGS="$F -C target-feature=+crt-static" cargo build --release --target $T && \
    cp target/$T/release/sessions /sessions

FROM scratch
COPY --from=build /sessions /sessions
CMD ["/sessions"]
