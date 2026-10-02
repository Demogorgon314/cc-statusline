#!/usr/bin/env python3
"""Generate README images from the real renderer and configurator with demo data.

Build first: cargo build --release --locked
Run: uv run --with pyte python scripts/screenshots.py

Requires Google Chrome (or SHOT_CHROME), ImageMagick and Hack Nerd Font Mono
(or SHOT_FONT). Uses an isolated Claude home and disposable Git repository.
The terminal frame is HTML; status lines and the PTY configurator are real.
Adapted from kimi-statusline's screenshot generator.
"""

import datetime
import html
import json
import os
import re
import shutil
import subprocess
import tempfile
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "target", "release", "cc-statusline")
ASSETS = os.path.join(ROOT, "assets")
CHROME = os.environ.get("SHOT_CHROME", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
FONT = os.environ.get("SHOT_FONT", "Hack Nerd Font Mono")

PAYLOAD = {}
TASKS = {}

XTERM16 = ["#1d1f21", "#e06c75", "#98c379", "#e5c07b", "#61afef", "#c678dd",
           "#56b6c2", "#dcdfe4", "#5c6370", "#ff7b86", "#b5e890", "#ffd88a",
           "#7cc4ff", "#de95f0", "#76d4e0", "#ffffff"]


def xterm256(n):
    if n < 16:
        return XTERM16[n]
    if n < 232:
        n -= 16
        step = [0, 95, 135, 175, 215, 255]
        return "#%02x%02x%02x" % (step[n // 36], step[n // 6 % 6], step[n % 6])
    v = 8 + (n - 232) * 10
    return "#%02x%02x%02x" % (v, v, v)


def setup_home():
    # macOS resolves /var to /private/var in getcwd(); preview keys must match.
    home = os.path.realpath(tempfile.mkdtemp(prefix="cc-statusline-shot-"))
    try:
        work = os.path.join(home, "demo", "code", "cc-statusline")
        os.makedirs(work)
        subprocess.run(["git", "init", "-q", "-b", "feat/tps", work], check=True)
        source = os.path.join(work, "demo.txt")
        with open(source, "w") as f:
            f.write("before\n" * 17)
        subprocess.run(["git", "-C", work, "add", "."], check=True)
        subprocess.run(["git", "-C", work, "-c", "user.name=Demo",
                        "-c", "user.email=demo@example.invalid", "-c", "commit.gpgsign=false",
                        "commit", "-qm", "Demo fixture"], check=True)
        with open(source, "w") as f:
            f.write("after\n" * 128)
        transcript = os.path.join(home, "projects", "demo", "session_demo.jsonl")
        agents = os.path.splitext(transcript)[0] + "/subagents"
        os.makedirs(agents)
        now = datetime.datetime.now(datetime.timezone.utc)
        def at(ago):
            return (now - datetime.timedelta(seconds=ago)).isoformat()
        def user(ago):
            return json.dumps({"type": "user", "sessionId": "session_demo", "timestamp": at(ago)}) + "\n"
        def message(ident, model, output, ago):
            usage = {"input_tokens": 2100, "output_tokens": output, "cache_read_input_tokens": 96000}
            if ident.startswith("main-"):
                # A recorded 1h cache write drives the usage countdown.
                usage.update({"cache_creation_input_tokens": 4000, "cache_creation":
                              {"ephemeral_1h_input_tokens": 4000, "ephemeral_5m_input_tokens": 0}})
            return json.dumps({"type": "assistant", "sessionId": "session_demo",
                "timestamp": at(ago), "message": {"id": ident, "model": model,
                "usage": usage}}) + "\n"
        # Timestamps drive TPS: two agents and the main loop are mid-request.
        with open(transcript, "w") as f:
            for i in range(12):
                f.write(user(3600 - i * 300) + message(f"main-{i}", "Opus", 3900, 3560 - i * 300))
            f.write(user(40) + message("main-next", "Opus", 900, 2))
        for agent, model, outputs in [("explore", "claude-sonnet-5-5", (1400, 1100)),
                                      ("review", "claude-haiku-4-5", (2400,)),
                                      ("plan", "claude-sonnet-5-5", (900,))]:
            done = agent == "plan"
            with open(os.path.join(agents, f"agent-{agent}.jsonl"), "w") as f:
                for i, output in enumerate(outputs):
                    end = 600 - i * 30 if done else 25 - i * 22
                    f.write(user(end + 20) + message(f"{agent}-{i}", model, output, end))
        TASKS.clear()
        TASKS.update({"session_id": "session_demo", "transcript_path": transcript, "tasks": [
            {"id": "explore", "name": "Explore", "model": "claude-sonnet-5-5", "status": "running",
             "tokenCount": 45900, "contextWindowSize": 200000},
            {"id": "review", "name": "code-reviewer", "model": "claude-haiku-4-5", "status": "running",
             "tokenCount": 171000, "contextWindowSize": 200000},
            {"id": "plan", "name": "Plan", "model": "claude-sonnet-5-5", "status": "completed",
             "tokenCount": 33500, "contextWindowSize": 1000000},
            {"id": "pending", "name": "general-purpose", "status": "pending"}]})
        PAYLOAD.clear()
        PAYLOAD.update({
            "session_id": "session_demo", "transcript_path": transcript,
            "workspace": {"current_dir": work},
            "model": {"display_name": "Opus 5.5 (1M context)"},
            "effort": {"level": "high"},
            "context_window": {"context_window_size": 1000000, "used_percentage": 9.7,
                              "current_usage": {"input_tokens": 1000, "cache_read_input_tokens": 96000}},
            "cost": {"total_cost_usd": 1.23, "total_duration_ms": 4380000,
                     "total_api_duration_ms": 200000, "total_lines_added": 128, "total_lines_removed": 17},
            "rate_limits": {
                "five_hour": {"used_percentage": 42, "resets_at": int(time.time()) + 4800},
                "seven_day": {"used_percentage": 63, "resets_at": int(time.time()) + 266400}},
            "pr": {"number": 42, "url": "https://example.invalid/demo/pull/42"}})
        return home
    except BaseException:
        shutil.rmtree(home)
        raise


def demo_env(home):
    env = dict(os.environ, CLAUDE_CONFIG_DIR=home, TERM="xterm-256color")
    for key in ("NO_COLOR", "CC_STATUSLINE_NO_COLOR", "CC_STATUSLINE_DEBUG"):
        env.pop(key, None)
    return env


def render(home, theme, width, light=False):
    # Colors follow Claude Code's theme setting, as on a real light terminal.
    with open(os.path.join(home, "settings.json"), "w") as f:
        json.dump({"theme": "light" if light else "dark"}, f)
    out = subprocess.run([BIN, "-t", theme, "--width", str(width)],
                         input=json.dumps(PAYLOAD), text=True, capture_output=True,
                         env=demo_env(home), check=True)
    return out.stdout.rstrip("\n")


def panel(home, width):
    out = subprocess.run([BIN, "subagents", "--width", str(width)], input=json.dumps(TASKS),
                         text=True, capture_output=True, env=demo_env(home), check=True)
    return [json.loads(line)["content"] for line in out.stdout.splitlines()]


def ansi_to_html(s, default_fg):
    s = re.sub(r"\x1b\]8;;[^\x07]*\x07", "", s)  # hyperlinks
    out, fg, bg, bold = [], None, None, False
    pos = 0
    for m in re.finditer(r"\x1b\[([0-9;]*)m", s):
        text = s[pos:m.start()]
        if text:
            style = []
            if fg:
                style.append("color:" + fg)
            if bg:
                style.append("background:" + bg)
            if bold:
                style.append("font-weight:700")
            out.append('<span style="%s">%s</span>' % (";".join(style), html.escape(text)))
        pos = m.end()
        codes = [int(c) if c else 0 for c in m.group(1).split(";")]
        i = 0
        while i < len(codes):
            c = codes[i]
            if c == 0:
                fg, bg, bold = None, None, False
            elif c == 1:
                bold = True
            elif c == 22:
                bold = False
            elif c == 39:
                fg = None
            elif c == 49:
                bg = None
            elif c == 2:
                fg = "#7f848e"
            elif 30 <= c <= 37:
                fg = XTERM16[c - 30]
            elif 90 <= c <= 97:
                fg = XTERM16[c - 90 + 8]
            elif 40 <= c <= 47:
                bg = XTERM16[c - 40]
            elif 100 <= c <= 107:
                bg = XTERM16[c - 100 + 8]
            elif c in (38, 48) and i + 1 < len(codes):
                if codes[i + 1] == 2 and i + 4 < len(codes):
                    col = "#%02x%02x%02x" % tuple(codes[i + 2:i + 5])
                    i += 4
                elif codes[i + 1] == 5 and i + 2 < len(codes):
                    col = xterm256(codes[i + 2])
                    i += 2
                else:
                    col = None
                if c == 38:
                    fg = col
                else:
                    bg = col
            i += 1
    tail = s[pos:]
    if tail:
        out.append(html.escape(tail))
    return "".join(out)


PAGE = """<!doctype html><meta charset="utf-8"><style>
body{margin:0;background:transparent;font-family:'%(font)s',monospace;font-size:15px}
.win{margin:18px;border-radius:10px;overflow:hidden;background:%(bg)s;
     box-shadow:0 10px 30px rgba(0,0,0,.35);display:inline-block;min-width:%(minw)spx}
.bar{height:30px;background:%(bar)s;display:flex;align-items:center;padding-left:12px;gap:8px}
.dot{width:12px;height:12px;border-radius:50%%}
.t{color:#8b8f98;font-family:-apple-system,sans-serif;font-size:12px;margin-left:12px}
.body{padding:14px 18px 16px;color:%(fg)s;white-space:pre;line-height:1.55}
.body.tight{line-height:1.2}
.dim{color:#6b6b6b}.label{color:#8b8f98;font-family:-apple-system,sans-serif;font-size:12px;
     margin:10px 0 4px}
.rule{border-top:1px solid %(rule)s;margin:6px 0}
</style><div class="win"><div class="bar">
<span class="dot" style="background:#ff5f57"></span><span class="dot" style="background:#febc2e"></span>
<span class="dot" style="background:#28c840"></span><span class="t">%(title)s</span></div>
<div class="body">%(body)s</div></div>"""


def page(body, title, light=False, minw=900):
    return PAGE % {"font": FONT, "bg": "#f6f6f6" if light else "#16181d",
                   "bar": "#e4e4e4" if light else "#22252b", "fg": "#1a1a1a" if light else "#e0e0e0",
                   "rule": "#d0d0d0" if light else "#2c2f36", "title": html.escape(title),
                   "body": body, "minw": minw}


def shoot(html_text, name, width):
    with tempfile.TemporaryDirectory(prefix="cc-statusline-chrome-") as tmp:
        src = os.path.join(tmp, "p.html")
        with open(src, "w") as f:
            f.write(html_text)
        raw = os.path.join(tmp, "raw.png")
        subprocess.run([CHROME, "--headless=new", "--disable-gpu", "--hide-scrollbars",
                        "--force-device-scale-factor=2", "--default-background-color=00000000",
                        "--no-first-run", "--no-default-browser-check", f"--window-size={width},1600",
                        f"--screenshot={raw}", "file://" + src],
                       capture_output=True, check=True, timeout=60)
        dst = os.path.join(ASSETS, name)
        subprocess.run(["magick", raw, "-trim", "+repage", "-bordercolor", "none", "-border", "24", dst], check=True)
    print("wrote", os.path.relpath(dst, ROOT))


def claude_footer(line):
    """Illustrative conversation frame around the actual renderer output."""
    return ('<span style="color:#d97757">❯</span> add output throughput to the status line\n'
            '<span class="dim">  Demo session · Opus 5.5 [1M] · high effort</span>\n'
            '<div class="rule"></div>' + line)


def main():
    os.makedirs(ASSETS, exist_ok=True)
    home = setup_home()
    try:
        hero = ansi_to_html(render(home, "powerline-light", 200, light=True), "#1a1a1a")
        shoot(page(claude_footer(hero), "Claude Code — demo session", light=True, minw=1180),
              "hero.png", 3000)

        rows = []
        for theme in ["claude", "cometix", "default", "minimal", "gruvbox", "nord", "powerline-dark",
                      "powerline-light", "powerline-rose-pine", "powerline-tokyo-night"]:
            line = ansi_to_html(render(home, theme, 205), "#e0e0e0")
            rows.append('<div class="label">%s</div>%s' % (theme, line))
        shoot(page("\n".join(rows), "cc-statusline themes", minw=1000), "themes.png", 2400)

        segs = []
        for w, label in [(300, "300 columns"), (150, "150 columns"), (110, "110 columns"), (80, "80 columns")]:
            segs.append('<div class="label">%s</div>%s' % (label, ansi_to_html(render(home, "claude", w), "")))
        shoot(page("\n".join(segs), "adaptive width", minw=1000), "adaptive.png", 3000)

        tree = '<span class="dim">  ⎿ </span>'
        agents = [claude_footer(ansi_to_html(render(home, "claude", 160), ""))]
        agents += [tree + ansi_to_html(row, "") for row in panel(home, 90)]
        for w in (60, 30):
            agents.append('<div class="label">agent panel at %d columns</div>' % w
                          + "\n".join(tree + ansi_to_html(row, "") for row in panel(home, w)))
        shoot(page("\n".join(agents), "parallel subagents", minw=1000), "subagents.png", 2400)
    finally:
        shutil.rmtree(home, ignore_errors=True)



def configurator_shot():
    """Drive `cc-statusline config` in a pty, replay the screen through
    pyte (a VT100 emulator), and render the cells as HTML."""
    import fcntl
    import pty
    import select
    import struct
    import termios

    import pyte

    cols, rows = 132, 40
    home = setup_home()
    pid, fd = None, None
    try:
        env = dict(demo_env(home), COLUMNS=str(cols), LINES=str(rows))
        work = PAYLOAD["workspace"]["current_dir"]
        env["PWD"] = work
        pid, fd = pty.fork()
        if pid == 0:
            os.chdir(work)
            os.execve(BIN, [BIN, "config"], env)
        fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        screen = pyte.Screen(cols, rows)
        stream = pyte.ByteStream(screen)

        def pump(t):
            end = time.time() + t
            while time.time() < end:
                r, _, _ = select.select([fd], [], [], 0.05)
                if r:
                    try:
                        stream.feed(os.read(fd, 1 << 16))
                    except OSError:
                        return

        pump(1.5)
        # select the Quota segment and open its settings
        for key in [b"\x1b[B"] * 10 + [b"\t"]:
            os.write(fd, key)
            pump(0.15)
        pump(0.5)
        body = screen_html(screen)
    finally:
        if pid:
            try:
                os.kill(pid, 9)
            except ProcessLookupError:
                pass
            os.waitpid(pid, 0)
        if fd is not None:
            os.close(fd)
        shutil.rmtree(home, ignore_errors=True)
    shoot(page(body, "cc-statusline config", minw=1000).replace('class="body"', 'class="body tight"'),
          "configurator.png", 2400)


PYTE_COLORS = {"black": 0, "red": 1, "green": 2, "brown": 3, "yellow": 3, "blue": 4, "magenta": 5,
               "cyan": 6, "white": 7, "brightblack": 8, "brightred": 9, "brightgreen": 10,
               "brightbrown": 11, "brightyellow": 11, "brightblue": 12, "brightmagenta": 13,
               "brightcyan": 14, "brightwhite": 15}


def pyte_color(c, fallback):
    if c == "default":
        return fallback
    if c in PYTE_COLORS:
        return XTERM16[PYTE_COLORS[c]]
    if re.fullmatch(r"[0-9a-fA-F]{6}", c):
        return "#" + c
    return fallback


def screen_html(screen):
    lines = []
    for y in range(screen.lines):
        row = screen.buffer[y]
        out, run, style = [], [], None
        for x in range(screen.columns):
            ch = row[x]
            fg = pyte_color(ch.fg, "#e0e0e0")
            bg = pyte_color(ch.bg, None)
            if ch.reverse:
                fg, bg = (bg or "#16181d"), fg
            st = "color:%s;%s%s" % (fg, "background:%s;" % bg if bg else "",
                                    "font-weight:700;" if ch.bold else "")
            if st != style and run:
                out.append('<span style="%s">%s</span>' % (style, html.escape("".join(run))))
                run = []
            style = st
            run.append(ch.data)
        if run:
            out.append('<span style="%s">%s</span>' % (style, html.escape("".join(run))))
        lines.append("".join(out))
    return "\n".join(lines)


if __name__ == "__main__":
    import pyte  # Fail before writing images if the PTY dependency is missing.
    main()
    configurator_shot()
