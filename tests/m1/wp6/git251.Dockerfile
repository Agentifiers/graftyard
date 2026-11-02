# The 2.51 client SHARED's git-http row names, for tests/m1/wp6/clients.sh: upstream git 2.51.0 from the kernel.org
# tarball, checked against its published SHA-256, unmodified.
# docker build -t graft-git251 -f tests/m1/wp6/git251.Dockerfile tests/m1/wp6
FROM debian:bookworm
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends build-essential curl ca-certificates \
      libcurl4-openssl-dev libexpat1-dev libssl-dev zlib1g-dev xz-utils >/dev/null && rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN curl -fsSLo git.tar.xz https://mirrors.edge.kernel.org/pub/software/scm/git/git-2.51.0.tar.xz \
 && echo "60a7c2251cc2e588d5cd87bae567260617c6de0c22dca9cdbfc4c7d2b8990b62  git.tar.xz" | sha256sum -c - \
 && tar -xJf git.tar.xz --strip-components=1 \
 && make -j"$(nproc)" prefix=/usr/local NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1 NO_GETTEXT=1 all \
 && make prefix=/usr/local NO_TCLTK=1 NO_PERL=1 NO_PYTHON=1 NO_GETTEXT=1 install \
 && cd / && rm -rf /src && git --version
