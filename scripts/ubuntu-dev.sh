#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE_NAME="${FORKTTY_UBUNTU_IMAGE:-forktty-ubuntu-dev}"
DOCKERFILE_PATH="$ROOT_DIR/docker/ubuntu-dev.Dockerfile"
HOST_USER="${FORKTTY_UBUNTU_USER:-${SUDO_USER:-$(stat -c %U "$ROOT_DIR")}}"
HOST_UID="${FORKTTY_UBUNTU_UID:-$(id -u "$HOST_USER")}"
HOST_GID="${FORKTTY_UBUNTU_GID:-$(id -g "$HOST_USER")}"
CONTAINER_HOME="/home/$HOST_USER"
CONTAINER_WORKDIR="/workspace/forktty"

build_image() {
  docker build \
    --build-arg UID="$HOST_UID" \
    --build-arg GID="$HOST_GID" \
    --build-arg USERNAME="$HOST_USER" \
    -t "$IMAGE_NAME" \
    -f "$DOCKERFILE_PATH" \
    "$ROOT_DIR"
}

if [[ "${FORKTTY_UBUNTU_REBUILD:-0}" == "1" ]] || ! docker image inspect "$IMAGE_NAME" >/dev/null 2>&1; then
  build_image
fi

tty_args=()
if [[ -t 0 && -t 1 ]]; then
  tty_args=(-it)
fi

docker_args=(
  run
  --rm
  "${tty_args[@]}"
  --user "$HOST_UID:$HOST_GID"
  -e HOME="$CONTAINER_HOME"
  -e CARGO_HOME="$CONTAINER_HOME/.cargo"
  -e RUSTUP_HOME="$CONTAINER_HOME/.rustup"
  -e npm_config_cache="$CONTAINER_HOME/.npm"
  -v "$ROOT_DIR:$CONTAINER_WORKDIR"
  -v "${IMAGE_NAME}-cargo:$CONTAINER_HOME/.cargo"
  -v "${IMAGE_NAME}-rustup:$CONTAINER_HOME/.rustup"
  -v "${IMAGE_NAME}-npm:$CONTAINER_HOME/.npm"
  -w "$CONTAINER_WORKDIR"
)

if [[ "${FORKTTY_UBUNTU_ENABLE_HOST_SESSION_BRIDGES:-0}" == "1" ]]; then
  if [[ -n "${XDG_RUNTIME_DIR:-}" && -d "${XDG_RUNTIME_DIR}" ]]; then
    docker_args+=(
      -e XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR"
      -v "${XDG_RUNTIME_DIR}:${XDG_RUNTIME_DIR}"
    )
  fi

  if [[ -n "${WAYLAND_DISPLAY:-}" ]]; then
    docker_args+=(-e WAYLAND_DISPLAY="$WAYLAND_DISPLAY")
  fi

  if [[ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]]; then
    docker_args+=(-e DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS")
  fi

  if [[ -n "${DISPLAY:-}" ]]; then
    docker_args+=(
      -e DISPLAY="$DISPLAY"
      -v /tmp/.X11-unix:/tmp/.X11-unix:ro
    )
  fi

  if [[ -n "${XAUTHORITY:-}" && -f "${XAUTHORITY}" ]]; then
    docker_args+=(
      -e XAUTHORITY="$XAUTHORITY"
      -v "${XAUTHORITY}:${XAUTHORITY}:ro"
    )
  fi
fi

if [[ -d /dev/dri ]]; then
  docker_args+=(--device /dev/dri)
fi

if [[ $# -eq 0 ]]; then
  set -- bash
fi

exec docker "${docker_args[@]}" "$IMAGE_NAME" "$@"
