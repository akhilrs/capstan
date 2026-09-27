#define _GNU_SOURCE
#include <sys/socket.h>
#include <sys/types.h>
#include <stdio.h>
#include <errno.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    struct ucred peer;
    socklen_t length = sizeof(peer);
    if (argc > 2 || getsockopt(3, SOL_SOCKET, SO_PEERCRED, &peer, &length) != 0 || length != sizeof(peer)) {
        fputs("Cannot authenticate Unix receipt peer\n", stderr);
        return 1;
    }
    if (argc == 2) {
        char *end = NULL;
        errno = 0;
        long expected = strtol(argv[1], &end, 10);
        return errno || !end || *end || expected < 0 || peer.pid != expected ? 1 : 0;
    }
    if (peer.pid <= 0 || printf("%ld\n", (long)peer.pid) < 0 || fflush(stdout) != 0) return 1;
    return 0;
}
