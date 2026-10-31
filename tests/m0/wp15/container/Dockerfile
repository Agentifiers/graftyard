# M0-WP10: git cache stand-in for graft-m0-agent. Alpine git plus gitd (gitd/main.go), as b0-22's image is built.
FROM golang:1.23-alpine AS build
WORKDIR /src
COPY gitd/ .
RUN CGO_ENABLED=0 go build -o /gitd .
FROM alpine:3.22
RUN apk add --no-cache git
COPY --from=build /gitd /gitd
RUN mkdir -p /data
EXPOSE 8080
CMD ["/gitd"]
