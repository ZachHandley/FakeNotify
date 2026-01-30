# FakeNotify DockerMod for linuxserver.io containers
#
# This mod automatically configures LD_PRELOAD for LSIO containers.
#
# Usage in docker-compose:
#   environment:
#     - DOCKER_MODS=ghcr.io/zachhandley/fakenotify-mod:latest
#     - FAKENOTIFY_SOCKET=/run/fakenotify/fakenotify.sock
#   volumes:
#     - /run/fakenotify:/run/fakenotify

FROM scratch

# The mod script
COPY root/ /
