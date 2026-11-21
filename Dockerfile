# Shopping Sessions: native Rust pipeline, static binary on an empty image.
FROM rust:1.91-bookworm AS build
WORKDIR /src
ARG TARGETARCH
ARG XCPU=x86-64-v3
# Scorers are Intel Xeon (Cascade Lake or newer): AVX2 + BMI2 are safe.
ENV CARGO_PROFILE_RELEASE_LTO=fat
# pgo.profdata is optional: a profile from a training run of this exact source (see
# OVERNIGHT_LOG.md). If the compiler cannot use it, the build falls back to a plain one.
COPY Cargo.toml Cargo.lock pgo.profdat[a] ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs && \
    P=""; if [ "$TARGETARCH" = "arm64" ]; then T=aarch64-unknown-linux-gnu; F="-C target-cpu=neoverse-n1"; else T=x86_64-unknown-linux-gnu; F="-C target-cpu=$XCPU"; if [ -f pgo.profdata ]; then P="-Cprofile-use=/src/pgo.profdata"; fi; fi && \
    (RUSTFLAGS="$F $P -C target-feature=+crt-static" cargo build --release --target $T || RUSTFLAGS="$F -C target-feature=+crt-static" cargo build --release --target $T) && rm -rf src
COPY src ./src
RUN touch src/main.rs && \
    P=""; if [ "$TARGETARCH" = "arm64" ]; then T=aarch64-unknown-linux-gnu; F="-C target-cpu=neoverse-n1"; else T=x86_64-unknown-linux-gnu; F="-C target-cpu=$XCPU"; if [ -f pgo.profdata ]; then P="-Cprofile-use=/src/pgo.profdata"; fi; fi && \
    (RUSTFLAGS="$F $P -C target-feature=+crt-static" cargo build --release --target $T || RUSTFLAGS="$F -C target-feature=+crt-static" cargo build --release --target $T) && \
    mkdir /out && cp target/$T/release/sessions /out/sessions
# A second build of the same source for CPUs with AVX-512 (it contains the vector line
# parser, src/xk). /sessions hands over to it at start-up when the CPU has AVX-512.
RUN if [ "$TARGETARCH" != "arm64" ]; then \
    RUSTFLAGS="-C target-cpu=x86-64-v4 -C target-feature=+crt-static" CARGO_TARGET_DIR=/src/target512 cargo build --release --target x86_64-unknown-linux-gnu && \
    cp target512/x86_64-unknown-linux-gnu/release/sessions /out/sessions512; fi

FROM scratch
COPY --from=build /out/ /
CMD ["/sessions"]
