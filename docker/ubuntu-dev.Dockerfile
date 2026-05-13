FROM ubuntu:24.04

ARG DEBIAN_FRONTEND=noninteractive
ARG UID=1000
ARG GID=1000
ARG USERNAME=builder
ARG NODE_MAJOR=22

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    build-essential \
    ca-certificates \
    curl \
    file \
    git \
    gnupg \
    libayatana-appindicator3-dev \
    libdbus-1-dev \
    libglib2.0-dev \
    librsvg2-dev \
    libssl-dev \
    libwebkit2gtk-4.1-dev \
    libxdo-dev \
    libgtk-3-dev \
    patchelf \
    pkg-config \
    unzip \
    wget \
    xz-utils \
    zip && \
    install -d -m 0755 /etc/apt/keyrings && \
    curl -fsSL https://deb.nodesource.com/gpgkey/nodesource-repo.gpg.key | gpg --dearmor -o /etc/apt/keyrings/nodesource.gpg && \
    echo "deb [signed-by=/etc/apt/keyrings/nodesource.gpg] https://deb.nodesource.com/node_${NODE_MAJOR}.x nodistro main" > /etc/apt/sources.list.d/nodesource.list && \
    apt-get update && \
    apt-get install -y --no-install-recommends nodejs && \
    rm -rf /var/lib/apt/lists/*

RUN existing_group="$(getent group "${GID}" | cut -d: -f1 || true)" && \
    if [ -z "$existing_group" ]; then groupadd --gid "${GID}" "${USERNAME}"; fi && \
    existing_user="$(getent passwd "${UID}" | cut -d: -f1 || true)" && \
    if [ -n "$existing_user" ] && [ "$existing_user" != "$USERNAME" ]; then \
      usermod --login "${USERNAME}" --home "/home/${USERNAME}" --move-home "$existing_user"; \
    fi && \
    if ! id -u "${USERNAME}" >/dev/null 2>&1; then \
      useradd --uid "${UID}" --gid "${GID}" --create-home --shell /bin/bash "${USERNAME}"; \
    else \
      usermod --gid "${GID}" "${USERNAME}"; \
    fi && \
    install -d -o "${UID}" -g "${GID}" "/home/${USERNAME}" /workspace

USER ${USERNAME}

ENV USER=${USERNAME}
ENV HOME=/home/${USERNAME}
ENV CARGO_HOME=/home/${USERNAME}/.cargo
ENV RUSTUP_HOME=/home/${USERNAME}/.rustup
ENV PATH=/home/${USERNAME}/.cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

RUN curl https://sh.rustup.rs -sSf | bash -s -- -y --profile minimal --default-toolchain stable

WORKDIR /workspace

CMD ["bash"]
