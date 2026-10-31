# M0-WP9 B0-34: git plus the shard server in wp9/git/main.go, so the shard object can stream R2 objects into the
# container's disk and run real git work on a real network.
FROM golang:1.23-alpine AS build
WORKDIR /src
COPY main.go go.mod ./
RUN CGO_ENABLED=0 go build -o /wp9git .
FROM alpine:3.22
RUN apk add --no-cache git
COPY --from=build /wp9git /wp9git
RUN mkdir -p /data
EXPOSE 8080
CMD ["/wp9git"]
