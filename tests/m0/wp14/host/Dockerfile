# M0-WP14 (L0-02): the stand-in Graftyard host's git backend. Alpine git plus hostd (hostd/main.go), a thin wrapper
# over `git http-backend` that seeds three bare repos (base, fork, other) and serves smart HTTP. Copied in spirit from
# tests/m0/wp16/Dockerfile. enableInternet is false; git bytes reach it only through the Durable Object.
FROM golang:1.23-alpine AS build
WORKDIR /src
COPY hostd/ .
RUN CGO_ENABLED=0 go build -o /hostd .
FROM alpine:3.22
RUN apk add --no-cache git git-daemon
COPY --from=build /hostd /hostd
RUN mkdir -p /data
EXPOSE 8080
CMD ["/hostd"]
