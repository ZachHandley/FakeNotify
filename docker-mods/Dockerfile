# FakeNotify DockerMod for linuxserver.io containers
#
# This mod automatically configures LD_PRELOAD for LSIO containers.
# It installs glibc compatibility for Alpine-based containers.
#
# Usage in docker-compose:
#   environment:
#     - DOCKER_MODS=ghcr.io/zachhandley/fakenotify-mod:latest
#   volumes:
#     - /run/fakenotify:/run/fakenotify:ro
#     - /usr/local/lib/libfakenotify_preload.so:/usr/local/lib/libfakenotify_preload.so:ro

FROM scratch

# The mod script
COPY root/ /
