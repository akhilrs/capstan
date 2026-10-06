#!/usr/bin/env python3
"""Key check for the Rust dashboard: drives `cstan dash` (which execs cstan-dash) in a pty against fake-daemon.mjs.

For each terminal size (80x24, 160x45) it sends every key the dashboard understands (1-5, tab, shift-tab, arrows,
f, y/s/c with confirm and cancel, e, o, p, r, -, +, ?, q), waits for the screen to settle, and asserts the expected
screen text or the expected daemon call. The transcript (one line per step, then the screen when a step fails) goes
to stdout; docs/research/rust-dash-keys.txt is that transcript.

Everything runs in a scratch project under /tmp with CAPSTAN_SOCKET, CAPSTAN_TOKEN and HERDR_* removed. The only
daemon is the scratch one this script starts: fake-daemon.mjs serves `status` (a captured status result, with the
first agents marked as just active), and a small logging proxy in front of it records every other call (`peek`,
`resolve`, `cancel`) and answers it. Nothing here touches a live controller; the only processes signalled are the
ones this script started.

usage: rust-keys.py --dash-bin PATH_TO_cstan-dash [--cli dist/src/cli.js] [--status STATUS.json] [--sizes 80x24,160x45]
Exit status 0 when every step passed.
"""
import argparse, codecs, fcntl, json, os, pty, re, select, shutil, signal, socket, struct, subprocess, sys, tempfile, termios
import threading, time, unicodedata

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
KEYS = {
    "tab": b"\t", "shift-tab": b"\x1b[Z", "up": b"\x1b[A", "down": b"\x1b[B", "esc": b"\x1b",
}


class Screen:
    """Just enough of a terminal for what ratatui's crossterm backend writes: cursor moves, text, clears, SGR."""

    def __init__(self, columns, rows):
        self.columns, self.rows = columns, rows
        self.cells = [[" "] * columns for _ in range(rows)]
        self.x = self.y = 0
        self.rest = ""
        self.decoder = codecs.getincrementaldecoder("utf-8")("ignore")
        self.alt = False
        self.bytes = 0

    def feed(self, data):
        self.bytes += len(data)
        text = self.rest + self.decoder.decode(data)
        self.rest = ""
        i = 0
        while i < len(text):
            c = text[i]
            if c == "\x1b":
                if i + 1 >= len(text):
                    self.rest = text[i:]
                    return
                if text[i + 1] == "[":
                    j = i + 2
                    while j < len(text) and not ("@" <= text[j] <= "~"):
                        j += 1
                    if j >= len(text):
                        self.rest = text[i:]
                        return
                    self.csi(text[i + 2:j], text[j])
                    i = j + 1
                    continue
                i += 2
                continue
            if c == "\r":
                self.x = 0
            elif c == "\n":
                self.y = min(self.y + 1, self.rows - 1)
            elif c < " ":
                pass
            else:
                width = 0 if unicodedata.combining(c) else (2 if unicodedata.east_asian_width(c) in "WF" else 1)
                if width and self.x < self.columns and self.y < self.rows:
                    self.cells[self.y][self.x] = c
                    if width == 2 and self.x + 1 < self.columns:
                        self.cells[self.y][self.x + 1] = ""
                self.x += width
            i += 1

    def csi(self, params, final):
        if final in "Hf":
            nums = [int(p) if p.isdigit() else 1 for p in params.split(";")] if params else [1, 1]
            nums += [1] * (2 - len(nums))
            self.y, self.x = max(nums[0] - 1, 0), max(nums[1] - 1, 0)
        elif final == "J" and params in ("", "2", "3", "0"):
            self.cells = [[" "] * self.columns for _ in range(self.rows)]
        elif final == "K":
            for k in range(self.x, self.columns):
                self.cells[self.y][k] = " "
        elif final in "hl" and params.startswith("?1049"):
            self.alt = final == "h"

    def lines(self):
        return ["".join(row).rstrip() for row in self.cells]

    def text(self):
        return "\n".join(self.lines())


class Proxy:
    """Listens where cstan expects the daemon; `ping` and `status` go to fake-daemon.mjs, the rest is logged and answered."""

    def __init__(self, path, upstream, agent_text):
        self.path, self.upstream, self.agent_text = path, upstream, agent_text
        self.calls = []
        self.statuses = 0
        self.lock = threading.Lock()
        self.server = socket.socket(socket.AF_UNIX)
        self.server.bind(path)
        self.server.listen(16)
        self.running = True
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while self.running:
            try:
                conn, _ = self.server.accept()
            except OSError:
                return
            threading.Thread(target=self.handle, args=(conn,), daemon=True).start()

    def handle(self, conn):
        with conn:
            data = b""
            while b"\n" not in data:
                chunk = conn.recv(65536)
                if not chunk:
                    return
                data += chunk
            request = json.loads(data.split(b"\n")[0])
            command, args = request.get("command"), request.get("args", [])
            if command in ("ping", "status"):
                if command == "status":
                    with self.lock:
                        self.statuses += 1
                up = socket.socket(socket.AF_UNIX)
                up.connect(self.upstream)
                up.sendall(data)
                reply = b""
                while not reply.endswith(b"\n"):
                    chunk = up.recv(1 << 20)
                    if not chunk:
                        break
                    reply += chunk
                up.close()
                if command == "ping":
                    reply = json.dumps({"ok": True, "requestId": "p", "result": {"pid": os.getpid()}}).encode() + b"\n"
                conn.sendall(reply)
                return
            with self.lock:
                self.calls.append((command, list(args), bool(request.get("credential"))))
            if command == "peek":
                result = {"agentStatus": "working", "text": self.agent_text}
            else:
                result = {"state": "cancelled" if command == "cancel" else "queued"}
            conn.sendall(json.dumps({"ok": True, "requestId": "r", "result": result}).encode() + b"\n")

    def take_calls(self):
        with self.lock:
            calls, self.calls = self.calls, []
        return calls

    def close(self):
        self.running = False
        self.server.close()


class Dash:
    def __init__(self, cmd, cwd, env, columns, rows):
        self.columns, self.rows = columns, rows
        self.screen = Screen(columns, rows)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, columns, 0, 0))
        self.proc = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=slave, stdout=slave, stderr=slave,
                                     close_fds=True, start_new_session=True)
        os.close(slave)

    def pump(self, seconds):
        end = time.time() + seconds
        while time.time() < end:
            ready, _, _ = select.select([self.master], [], [], max(end - time.time(), 0))
            if not ready:
                break
            try:
                data = os.read(self.master, 65536)
            except OSError:
                return False
            if not data:
                return False
            self.screen.feed(data)
        return True

    def settle(self, quiet=0.35, limit=4.0):
        """Reads until the output has been quiet for `quiet` seconds."""
        end = time.time() + limit
        while time.time() < end:
            before = self.screen.bytes
            self.pump(quiet)
            if self.screen.bytes == before:
                return True
        return False

    def wait_for(self, predicate, timeout=3.0):
        """Reads until predicate(screen text) holds; True when it did within the timeout."""
        end = time.time() + timeout
        while True:
            if predicate(self.screen.text()):
                return True
            if time.time() >= end or not self.pump(0.1):
                return predicate(self.screen.text())

    def send(self, data):
        os.write(self.master, data)

    def alive(self):
        return self.proc.poll() is None


class Run:
    def __init__(self, name):
        self.name, self.failures, self.steps = name, 0, 0

    def check(self, label, ok, dash=None, detail=""):
        self.steps += 1
        if ok:
            print(f"  ok    {label}")
        else:
            self.failures += 1
            print(f"  FAIL  {label} {detail}")
            if dash is not None:
                print("  ---- screen ----")
                for line in dash.screen.lines():
                    print("  | " + line)
                print("  ----------------")


def prepare(args):
    work = tempfile.mkdtemp(prefix="cstk", dir="/tmp")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CAPSTAN_", "HERDR_", "CSTAN_"))}
    env.update(TERM="xterm-256color", LANG="C.UTF-8", HOME=work + "/home", CAPSTAN_LAUNCH="off")
    env["PATH"] = "/usr/bin:/bin:" + os.path.dirname(shutil.which("node") or "/usr/bin/node")
    os.makedirs(env["HOME"])
    project = work + "/p"
    os.makedirs(project)
    subprocess.run(["git", "init", "-q", "."], cwd=project, check=True, env=env)
    subprocess.run(["git", "-c", "user.name=k", "-c", "user.email=k@example.invalid", "commit", "-q", "--allow-empty", "-m", "i"],
                   cwd=project, check=True, env=env)
    subprocess.run(["node", args.cli, "init"], cwd=project, check=True, env=env, stdout=subprocess.DEVNULL)
    status = json.load(open(args.status)) if args.status.endswith(".json") else None
    if "status" in status and "agents" not in status:
        status = status["status"]
    # Eight more ended agents, so that `e` (show every ended agent, not the 5 latest) changes the agents panel.
    for n in range(1, 9):
        status["agents"].append({"agentId": f"ended-{n}", "roleName": "developer", "kind": "Developer", "generation": 1,
                                 "state": "ended", "lastActivityAt": f"2026-10-02T11:{10 + n}:00.000Z"})
    status_file = work + "/status.json"
    json.dump(status, open(status_file, "w"))
    return work, project, env, status_file


SUP = "¹²³⁴⁵"
UP, DOWN, TAB, BTAB, ESC = b"\x1b[A", b"\x1b[B", b"\t", b"\x1b[Z", b"\x1b"
MESSAGE = "0192f4c1-3a7e-7b2d"  # #41, unacked: retry, skip and cancel apply
QUEUED_MESSAGE = "0192f4c9-61d0-7c13"  # #42, queued: only skip and cancel apply


def focus_of(text):
    """The panel with the heavy border, 1 to 5 (the heavy corner is followed by the panel's superscript number)."""
    for line in text.split("\n"):
        for i, c in enumerate(line[:-2]):
            if c == "\u250f" and line[i + 1] == "\u2501" and line[i + 2] in SUP:
                return SUP.index(line[i + 2]) + 1
    return None


def shown_panels(text):
    found = set()
    for line in text.split("\n"):
        for i, c in enumerate(line[:-1]):
            if c in "\u256d\u250f" and line[i + 1] in "\u2500\u2501" and i + 2 < len(line) and line[i + 2] in SUP:
                found.add(SUP.index(line[i + 2]) + 1)
    return sorted(found)


def selected_row(text):
    """The text of the row carrying the selection mark in the focused panel."""
    for line in text.split("\n"):
        if "\u2503\u258c" in line:
            return line
    return ""


def footer(text):
    lines = [line for line in text.split("\n") if line.strip()]
    return lines[-1] if lines else ""


def agents_total(text):
    """The `n` of the agents panel's `a-b/n` range label, on the bottom border that carries `working: inferred`."""
    for line in text.split("\n"):
        m = re.search(r"working: inferred.*?\u2524 (?:\d+-\d+|\d+)/(\d+) \u251c", line)
        if m:
            return int(m.group(1))
    return None


def drive_keys(run, dash, proxy, args):
    def press(label, data, predicate, calls=None, timeout=3.0, settle=0.0):
        """Sends a key, waits for the predicate, then checks the daemon calls made since the last step."""
        proxy.take_calls()
        dash.send(data)
        ok = dash.wait_for(predicate, timeout)
        if settle:
            dash.pump(settle)
        got = [(c, a) for c, a, _ in proxy.take_calls()]
        want = calls or []
        run.check(label, ok and got == want, dash, f"daemon calls {got} (wanted {want})" if got != want else "")
        for call, call_args in got:
            print(f"          daemon call: {call} {' '.join(call_args)}")

    text = lambda: dash.screen.text()
    shown = shown_panels(text())
    run.check(f"panels shown at {dash.columns}x{dash.rows}: {shown}", len(shown) >= 3, dash)
    run.check("the queue has the focus at the start (hint line: y retry)", focus_of(text()) == 3 and "y retry" in footer(text()), dash)

    # 1-5 jump to a panel when it is shown, else the focus stays.
    for n in range(1, 6):
        before = focus_of(text())
        want = n if n in shown else before
        press(f"key {n}: focus {'moves to' if n in shown else 'stays (panel not shown) on'} panel {want}", str(n).encode(),
              lambda t, w=want: focus_of(t) == w, settle=0.2)
    # tab and shift-tab walk the shown panels, with wrap-around.
    dash.send(b"1"); dash.wait_for(lambda t: focus_of(t) == 1)
    for expect in shown[1:] + shown[:1]:
        press(f"tab: focus {expect}", TAB, lambda t, e=expect: focus_of(t) == e)
    for expect in list(reversed(shown))[0:1] + list(reversed(shown))[1:]:
        press(f"shift-tab: focus {expect}", BTAB, lambda t, e=expect: focus_of(t) == e)
    # Now on panel 1 again; the queue next.
    press("key 3: queue focused", b"3", lambda t: focus_of(t) == 3 and "y retry" in footer(t))

    # arrows (and j k) move the queue selection: #41, #42, #44.
    press("down: selection on #42", DOWN, lambda t: "#42" in selected_row(t))
    press("down: selection on #44", DOWN, lambda t: "#44" in selected_row(t))
    press("up: selection back on #42", UP, lambda t: "#42" in selected_row(t))
    press("k: selection on #41", b"k", lambda t: "#41" in selected_row(t))
    press("j: selection on #42", b"j", lambda t: "#42" in selected_row(t))
    # a decision that cannot apply to a queued message is refused on the spot
    press("y on a queued message: refused with a notice, no prompt, no call", b"y",
          lambda t: "retry does not apply to a message" in t and "Retry message" not in t)
    press("up: selection on #41 again", UP, lambda t: "#41" in selected_row(t))

    # f: problems only
    press("f: problems only is on, #42 leaves the list", b"f", lambda t: "f problems only [x]" in t and "#42" not in t)
    press("f: problems only is off, #42 is back", b"f", lambda t: "f problems only [ ]" in t and "#42" in t)

    # y / s / c: prompt, cancel, confirm
    for key, word, call in (
        (b"y", "Retry", ("resolve", [MESSAGE, "retry"])),
        (b"s", "Skip", ("resolve", [MESSAGE, "skip"])),
        (b"c", "Cancel", ("cancel", [MESSAGE])),
    ):
        k = key.decode()
        press(f"{k}: asks to {word.lower()} #41 (nothing sent yet)", key, lambda t, w=word: f"{w} message {MESSAGE}" in t)
        press(f"{k} then n: cancelled, no call", b"n", lambda t, w=word: "cancelled" in t and f"{w} message" not in t)
        press(f"{k} then esc: cancelled, no call", key, lambda t, w=word: f"{w} message {MESSAGE}" in t)
        press(f"{k}, esc: cancelled, no call", ESC, lambda t, w=word: f"{w} message" not in t and "cancelled" in t)
        press(f"{k}: asks again", key, lambda t, w=word: f"{w} message {MESSAGE}" in t)
        dash.pump(0.45)  # the confirmation is ignored for the first 300 ms
        verb = {"Retry": "retry", "Skip": "skip", "Cancel": "cancel"}[word]
        press(f"{k}, y: confirmed, the daemon gets {call[0]} {' '.join(call[1])}", b"y",
              lambda t, v=verb: f"{v} {MESSAGE}: " in t, calls=[call])
        dash.pump(1.2)
    return finish_keys(run, dash, proxy, args, press, shown)


def finish_keys(run, dash, proxy, args, press, shown):
    text = lambda: dash.screen.text()
    # agents: e, o
    press("key 1: agents focused (hint: o observe)", b"1", lambda t: focus_of(t) == 1 and "o observe" in footer(t))
    total = agents_total(text())
    run.check(f"agents panel lists {total} rows (5 active + the 5 latest ended)", total == 10, dash, f"total={total}")
    press("e: every ended agent is listed", b"e", lambda t: agents_total(t) == 14)
    press("e: back to the 5 latest ended", b"e", lambda t: agents_total(t) == 10)
    press("o on developer-2 (active): the daemon gets peek developer-2 40, the overlay opens", b"o",
          lambda t: "observe developer-2" in t and "esc or q closes" in t, calls=[("peek", ["developer-2", "40"])])
    press("q inside the overlay closes it and does not quit", b"q", lambda t: "esc or q closes" not in t)
    run.check("the dashboard is still running", dash.alive(), dash)
    press("o again, esc closes the overlay", b"o", lambda t: "esc or q closes" in t, calls=[("peek", ["developer-2", "40"])])
    press("esc", ESC, lambda t: "esc or q closes" not in t)
    for n in range(5):
        dash.send(DOWN)
        dash.pump(0.15)
    press("o on an ended agent: the footer says it has ended, no call", b"o", lambda t: "has ended; observe needs an" in t)
    press("up x5 back to the first agent", UP * 5, lambda t: "developer-2" in selected_row(t))

    # p: pause; actions are off while paused
    press("p: PAUSED in the header", b"p", lambda t: "PAUSED" in t)
    press("3 then y while paused: no prompt", b"3y", lambda t: focus_of(t) == 3 and "Retry message" not in t, settle=0.4)
    press("p: resumed", b"p", lambda t: "PAUSED" not in t)

    # + - and r
    for n in range(2, 6):
        press(f"+: interval {n}s", b"+", lambda t, n=n: f"- {n}s +" in t)
    before = proxy.statuses
    time.sleep(0.2)
    before = proxy.statuses
    dash.send(b"r")
    ok = False
    end = time.time() + 1.5
    while time.time() < end:
        dash.pump(0.1)
        if proxy.statuses > before:
            ok = True
            break
    run.check("r: polls at once (a status request within 1.5 s with a 5 s interval)", ok, dash, f"statuses {before} -> {proxy.statuses}")
    for n in range(4, 0, -1):
        press(f"-: interval {n}s", b"-", lambda t, n=n: f"- {n}s +" in t)
    press("- at 1s stays at 1s", b"-", lambda t: "- 1s +" in t, settle=0.3)

    # ? help
    press("?: help overlay", b"?", lambda t: "NAVIGATE" in t and "? or esc closes" in t)
    press("? again closes it", b"?", lambda t: "NAVIGATE" not in t)
    press("? then esc closes it", b"?", lambda t: "NAVIGATE" in t)
    press("esc", ESC, lambda t: "NAVIGATE" not in t)
    press("? then another key closes it", b"?", lambda t: "NAVIGATE" in t)
    press("x", b"x", lambda t: "NAVIGATE" not in t)


def run_size(args, work, project, env, proxy, columns, rows):
    run = Run(f"{columns}x{rows}")
    print(f"== {columns}x{rows}")
    denv = dict(env, CSTAN_DASH="rust", CSTAN_DASH_BIN=os.path.abspath(args.dash_bin))
    dash = Dash(["node", args.cli, "dash", "--interval", "1"], project, denv, columns, rows)
    try:
        run.check("first frame draws (cstan dash -> cstan-dash)", dash.wait_for(lambda t: "agents" in t and "queue" in t, 10), dash)
        exe = os.path.basename(os.path.realpath(f"/proc/{dash.proc.pid}/exe"))
        run.check("the process is cstan-dash (same pid as cstan dash)", exe == "cstan-dash", dash, f"exe={exe}")
        drive_keys(run, dash, proxy, args)
        dash.send(b"q")
        dash.pump(1.5)
        run.check("q quits with exit status 0", dash.proc.poll() == 0, dash, f"status={dash.proc.poll()}")
    finally:
        if dash.alive():
            os.killpg(dash.proc.pid, signal.SIGKILL)
        os.close(dash.master)
    # ctrl+c quits too
    dash = Dash(["node", args.cli, "dash"], project, denv, columns, rows)
    try:
        dash.wait_for(lambda t: "agents" in t, 10)
        dash.send(b"\x03")
        dash.pump(1.5)
        run.check("ctrl+c quits with exit status 0", dash.proc.poll() == 0, dash, f"status={dash.proc.poll()}")
    finally:
        if dash.alive():
            os.killpg(dash.proc.pid, signal.SIGKILL)
        os.close(dash.master)
    print(f"  {run.steps - run.failures}/{run.steps} steps passed")
    return run


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dash-bin", required=True)
    ap.add_argument("--cli", default=os.path.join(ROOT, "dist/src/cli.js"))
    ap.add_argument("--status", default=os.path.join(ROOT, "dash/tests/parity/showcase.json"))
    ap.add_argument("--sizes", default="80x24,160x45")
    args = ap.parse_args()
    work, project, env, status_file = prepare(args)
    started = []
    try:
        real = work + "/real.sock"
        fake = subprocess.Popen(["node", os.path.join(HERE, "fake-daemon.mjs"), real, status_file, "2"], env=env)
        started.append(fake)
        for _ in range(50):
            if os.path.exists(real):
                break
            time.sleep(0.1)
        state = os.path.join(project, ".capstan", "state")
        os.makedirs(state, exist_ok=True)
        proxy = Proxy(os.path.join(state, "control.sock"), real, "developer-1 pane p3\nReading docs\nEdit(src/a.ts)\n")
        version = subprocess.run([args.dash_bin, "--version"], capture_output=True, text=True).stdout.strip()
        print(f"rust-keys.py: {version}, node {subprocess.run(['node', '-v'], capture_output=True, text=True, env=env).stdout.strip()}, "
              f"{time.strftime('%Y-%m-%d')}, sizes {args.sizes}")
        failures = 0
        for size in args.sizes.split(","):
            columns, rows = (int(n) for n in size.split("x"))
            failures += run_size(args, work, project, env, proxy, columns, rows).failures
        print("failed steps: %d" % failures)
        return 1 if failures else 0
    finally:
        for p in started:
            p.terminate()
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
