# M0-WP16 predictor shard: git plus the shard server (wp16/shard/main.go), copied in spirit from b0-22/Dockerfile.
FROM golang:1.23-alpine AS build
WORKDIR /src
COPY shard/ .
RUN CGO_ENABLED=0 go build -o /shard .
FROM alpine:3.22
RUN apk add --no-cache git
COPY --from=build /shard /shard
RUN mkdir -p /data
EXPOSE 8080
CMD ["/shard"]
