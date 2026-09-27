#define _GNU_SOURCE
#include <errno.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <poll.h>
#include <unistd.h>

static int matches_process(pid_t pid, const char *expected_start, const char *expected_script) {
    char path[64];
    snprintf(path, sizeof(path), "/proc/%ld/stat", (long)pid);
    FILE *stat_file = fopen(path, "r");
    if (!stat_file) return errno == ENOENT ? 0 : -1;
    char line[4096];
    if (!fgets(line, sizeof(line), stat_file)) { fclose(stat_file); return -1; }
    fclose(stat_file);
    char *field = strrchr(line, ')');
    if (!field || field[1] != ' ') return -1;
    field += 2;
    char *save = NULL;
    char *token = strtok_r(field, " ", &save);
    for (int number = 3; token && number < 22; number++) token = strtok_r(NULL, " ", &save);
    if (!token || strcmp(token, expected_start) != 0) return 0;

    snprintf(path, sizeof(path), "/proc/%ld/cmdline", (long)pid);
    FILE *cmd_file = fopen(path, "r");
    if (!cmd_file) return errno == ENOENT ? 0 : -1;
    char cmdline[65536];
    size_t length = fread(cmdline, 1, sizeof(cmdline) - 1, cmd_file);
    int read_error = ferror(cmd_file);
    fclose(cmd_file);
    if (read_error || length == 0) return -1;
    cmdline[length] = '\0';
    size_t first = strnlen(cmdline, length);
    if (first >= length || first + 1 >= length) return 0;
    char *script = cmdline + first + 1;
    size_t script_length = strnlen(script, length - first - 1);
    if (script_length != strlen(expected_script) || memcmp(script, expected_script, script_length) != 0) return 0;
    if (first + 1 + script_length + 1 >= length) return 0;
    char *action = script + script_length + 1;
    size_t action_length = strnlen(action, length - (size_t)(action - cmdline));
    if (action_length != 5 || memcmp(action, "serve", 5) != 0) return 0;
    return 1;
}

int main(int argc, char **argv) {
    if (argc != 5 || (strcmp(argv[1], "--check") != 0 && strcmp(argv[1], "--terminate") != 0)) {
        fputs("Usage: m1-egress-kill --check|--terminate PID START_TICKS SCRIPT\n", stderr);
        return 2;
    }
    char *end = NULL;
    errno = 0;
    long raw_pid = strtol(argv[2], &end, 10);
    if (errno || !end || *end || raw_pid <= 0) return 2;
    pid_t pid = (pid_t)raw_pid;
    int pidfd = (int)syscall(SYS_pidfd_open, pid, 0);
    if (pidfd < 0) return errno == ESRCH ? (strcmp(argv[1], "--check") == 0 ? 3 : 0) : 1;
    int match = matches_process(pid, argv[3], argv[4]);
    if (match <= 0) { close(pidfd); return match == 0 ? (strcmp(argv[1], "--check") == 0 ? 3 : 0) : 1; }
    if (strcmp(argv[1], "--check") == 0) { close(pidfd); return 0; }
    int result = (int)syscall(SYS_pidfd_send_signal, pidfd, SIGTERM, NULL, 0);
    int signal_error = errno;
    if (result != 0 && signal_error != ESRCH) {
        close(pidfd);
        errno = signal_error;
        perror("pidfd_send_signal");
        return 1;
    }
    struct pollfd descriptor = { .fd = pidfd, .events = POLLIN };
    int exited = poll(&descriptor, 1, 5000);
    if (exited == 0) {
        if (syscall(SYS_pidfd_send_signal, pidfd, SIGKILL, NULL, 0) != 0 && errno != ESRCH) {
            perror("pidfd_send_signal SIGKILL");
            close(pidfd);
            return 1;
        }
        exited = poll(&descriptor, 1, 5000);
    }
    close(pidfd);
    if (exited <= 0) {
        if (exited < 0) perror("poll pidfd");
        else fputs("Timed out waiting for egress proxy exit\n", stderr);
        return 1;
    }
    return 0;
}
