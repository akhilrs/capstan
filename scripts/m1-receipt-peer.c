#define _GNU_SOURCE
#include <sys/socket.h>
#include <sys/types.h>
#include <stdio.h>

int main(void) {
    struct ucred peer;
    socklen_t length = sizeof(peer);
    if (getsockopt(3, SOL_SOCKET, SO_PEERCRED, &peer, &length) != 0 || length != sizeof(peer) || peer.pid <= 0) {
        fputs("Cannot authenticate Unix receipt peer\n", stderr);
        return 1;
    }
    if (printf("%ld\n", (long)peer.pid) < 0 || fflush(stdout) != 0) return 1;
    return 0;
}
