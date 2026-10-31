# B0-16 test image: the smallest image that proves a container deploy. Upstream git, as the git cache shard image runs.
FROM alpine:3.22
RUN apk add --no-cache git busybox-extras
RUN mkdir -p /www && git --version > /www/index.html && echo v2 > /www/v
EXPOSE 8080
CMD ["httpd", "-f", "-p", "8080", "-h", "/www"]
