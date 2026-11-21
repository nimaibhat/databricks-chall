FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN sh /src/build.sh

FROM scratch
COPY --from=build /out/ /
USER 1000:1000
ENTRYPOINT ["/probe"]
