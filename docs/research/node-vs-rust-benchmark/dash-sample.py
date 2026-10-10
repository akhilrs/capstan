#!/usr/bin/env python3
"""Runs a dashboard binary in a 160x45 pty against a live daemon socket and samples its CPU and RSS from /proc.
usage: dash-sample.py <seconds> <socket> <cmd...>; the operator credential comes in BENCH_DASH_CREDENTIAL (cstan-dash reads it as CSTAN_DASH_CREDENTIAL)
Prints: cpu_percent=<one core> rss_kb=<VmRSS at end> hwm_kb=<VmHWM>. Only the child it starts is signalled."""
import os, pty, sys, time, fcntl, termios, struct, select, signal

seconds = float(sys.argv[1]); cmd = sys.argv[3:]
home = os.environ.get("BENCH_HOME", "/tmp"); credential = os.environ.get("BENCH_DASH_CREDENTIAL", "")
extra = {k[6:]: v for k, v in os.environ.items() if k.startswith("BENCH_X_")}  # BENCH_X_FOO=1 -> FOO=1 in the child
pid, fd = pty.fork()
if pid == 0:
    os.environ.clear()
    os.environ.update({"HOME": home, "PATH": "/usr/bin:/bin", "TERM": "xterm-256color",
                       "CAPSTAN_LAUNCH": "off",
                       **extra})
    if credential:
        os.environ["CSTAN_DASH_CREDENTIAL"] = credential
    os.execv(cmd[0], cmd)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 160, 0, 0))
def ticks():
    with open(f"/proc/{pid}/stat") as f:
        rest = f.read().rsplit(") ", 1)[1].split()
    return int(rest[11]) + int(rest[12])
def status(key):
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith(key + ":"):
                return int(line.split()[1])
def drain(until):
    while time.time() < until:
        r, _, _ = select.select([fd], [], [], 0.2)
        if r:
            try: os.read(fd, 65536)
            except OSError: return
time.sleep(0)
drain(time.time() + 5)           # settle
t0 = ticks(); w0 = time.time()
drain(w0 + seconds)
t1 = ticks(); w1 = time.time()
print(f"cpu_percent={(t1 - t0) / 100 / (w1 - w0) * 100:.2f} rss_kb={status('VmRSS')} hwm_kb={status('VmHWM')}")
os.kill(pid, signal.SIGTERM)
try: os.waitpid(pid, 0)
except ChildProcessError: pass
