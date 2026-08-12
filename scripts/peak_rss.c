/* Tiny fork+exec parent for wall time + child peak RSS.
 *
 * Python subprocess fork COW briefly inherits the interpreter's RSS into
 * ru_maxrss before exec; measuring from a small C parent avoids that.
 *
 * Usage: peak_rss cmd [args...]
 * Prints: "<elapsed_s> <peak_rss_kb>" on stdout; discards child stdout/stderr.
 * Exit status mirrors the child (or 127 if exec fails).
 */
#define _GNU_SOURCE
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
  if (argc < 2) {
    fprintf(stderr, "usage: %s cmd [args...]\n", argv[0]);
    return 2;
  }

  struct timespec t0, t1;
  clock_gettime(CLOCK_MONOTONIC, &t0);

  pid_t pid = fork();
  if (pid < 0) {
    perror("fork");
    return 1;
  }
  if (pid == 0) {
    int devnull = open("/dev/null", O_RDWR);
    if (devnull >= 0) {
      dup2(devnull, STDIN_FILENO);
      dup2(devnull, STDOUT_FILENO);
      dup2(devnull, STDERR_FILENO);
      if (devnull > STDERR_FILENO) {
        close(devnull);
      }
    }
    execvp(argv[1], argv + 1);
    _exit(127);
  }

  int st = 0;
  struct rusage ru;
  if (wait4(pid, &st, 0, &ru) < 0) {
    perror("wait4");
    return 1;
  }
  clock_gettime(CLOCK_MONOTONIC, &t1);
  double sec =
      (double)(t1.tv_sec - t0.tv_sec) + (double)(t1.tv_nsec - t0.tv_nsec) / 1e9;

  /* Linux: kilobytes; macOS: bytes. */
#ifdef __APPLE__
  long rss_kb = ru.ru_maxrss / 1024;
#else
  long rss_kb = ru.ru_maxrss;
#endif
  printf("%.6f %ld\n", sec, rss_kb);
  if (WIFEXITED(st)) {
    return WEXITSTATUS(st);
  }
  return 1;
}
