# The git cache shard image (design/DESIGN.md, Stack, Containers; Shape, "Git cache shards (derived)"): upstream git
# built from the kernel.org release tarball, checked against its published SHA-256, plus the shard server
# (server/main.go). Built and pushed by wrangler from the shard script's config (tests/m1/wp5/wrangler.jsonc), or by
# `docker build containers/git`.
ARG GIT_VERSION=2.56.0
ARG GIT_SHA256=26c56c296b38c0695b26fa95f475f1d01704d2d38e73465ca30b0b2f5dc789d3

FROM alpine:3.22 AS git
ARG GIT_VERSION
ARG GIT_SHA256
RUN apk add --no-cache build-base linux-headers cargo curl-dev expat-dev openssl-dev pcre2-dev zlib-dev perl gettext-dev xz curl
# musl's regex lacks REG_STARTEND, so git uses its own, as Alpine's package does. The Rust parts build too, as
# upstream builds them by default (Makefile, "Optional Rust support").
ENV GIT_MAKE="prefix=/usr/local NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1 NO_GETTEXT=1 USE_LIBPCRE2=1 NO_REGEX=NeedsStartEnd INSTALL_SYMLINKS=1"
WORKDIR /src
RUN curl -fsSLo git.tar.xz "https://mirrors.edge.kernel.org/pub/software/scm/git/git-${GIT_VERSION}.tar.xz" \
 && echo "${GIT_SHA256}  git.tar.xz" | sha256sum -c - \
 && tar -xJf git.tar.xz --strip-components=1 \
 && make -j"$(nproc)" $GIT_MAKE all && make $GIT_MAKE DESTDIR=/out install \
 && rm -rf /out/usr/local/share/man /out/usr/local/share/gitweb /out/usr/local/share/perl5 \
 && for f in $(find /out/usr/local -type f -perm -u+x); do \
      if [ "$(head -c 4 "$f" | od -An -tx1 | tr -d ' ')" = 7f454c46 ]; then strip --strip-unneeded "$f"; fi; done

FROM golang:1.23-alpine AS server
WORKDIR /src
COPY server/ .
RUN CGO_ENABLED=0 go build -trimpath -ldflags="-s -w" -o /shard-server .

FROM alpine:3.22
RUN apk add --no-cache zlib pcre2 expat libcurl ca-certificates
COPY --from=git /out/usr/local /usr/local
COPY --from=server /shard-server /usr/local/bin/shard-server
RUN mkdir -p /data && git --version
EXPOSE 8080
CMD ["/usr/local/bin/shard-server"]
