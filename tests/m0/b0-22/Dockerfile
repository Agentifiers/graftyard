# B0-22: git plus a static PUT sink, so the shard object can stream R2 objects into the container's disk.
FROM golang:1.23-alpine AS build
WORKDIR /src
COPY sink/ .
RUN CGO_ENABLED=0 go build -o /sink .
FROM alpine:3.22
RUN apk add --no-cache git
COPY --from=build /sink /sink
RUN mkdir -p /data
EXPOSE 8080
CMD ["/sink"]
