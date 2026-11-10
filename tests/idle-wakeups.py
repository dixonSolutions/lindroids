#!/usr/bin/env python3
"""Battery regressions: how often the server wakes up, and how fast a program draws for nobody.

cargo build -p lindroids
python3 tests/idle-wakeups.py [target/debug/lindroids]

The debug app stands in for the page; a tiny wl_shm client (this script, run as `--client`)
draws once (idle) or on every frame callback (animating). Wakeups are the voluntary context
switches of the server's threads. Checks:
  1. idle, with a page and an idle window: under one wakeup a second;
  2. the page gone (killed in the background): an animating program gets about one frame
     callback a second, and is told it is suspended (xdg_toplevel v6);
  3. a page linking again gets the window's frame at once, and the program is no longer suspended.
"""
import json, os, signal, socket, struct, subprocess, sys, tempfile, time, array, mmap
from pathlib import Path


def client(mode, out):
    sock = socket.socket(socket.AF_UNIX)
    sock.connect(os.path.join(os.environ['XDG_RUNTIME_DIR'], os.environ['WAYLAND_DISPLAY']))
    u = lambda *xs: struct.pack('=' + 'I' * len(xs), *xs)
    def string(t):
        d = t.encode() + b'\0'
        return u(len(d)) + d + b'\0' * (-len(d) % 4)
    def send(obj, op, body=b'', fd=None):
        data = u(obj, ((len(body) + 8) << 16) | op) + body
        if fd is None: sock.sendall(data)
        else: sock.sendmsg([data], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])
    buf = b''
    def event():
        nonlocal buf
        while len(buf) < 8 or len(buf) < struct.unpack_from('=I', buf, 4)[0] >> 16:
            d = sock.recv(65536)
            if not d: raise SystemExit('disconnected')
            buf += d
        obj, h = struct.unpack_from('=II', buf); size = h >> 16
        body, buf = buf[8:size], buf[size:]
        return obj, h & 65535, body
    send(1, 1, u(2)); send(1, 0, u(3))
    g = {}; vers = {}
    while True:
        o, op, b = event()
        if o == 3: break
        if o == 2 and op == 0:
            name, ln = struct.unpack_from('=II', b); iface = b[8:8 + ln - 1].decode()
            g[iface] = name; vers[iface] = struct.unpack_from('=I', b, 8 + ((ln + 3) & ~3))[0]
    xv = min(6, vers['xdg_wm_base'])
    for iface, ver, ident in [('wl_compositor', 4, 4), ('wl_shm', 1, 5), ('xdg_wm_base', xv, 6)]:
        send(2, 0, u(g[iface]) + string(iface) + u(ver, ident))
    W, H = 320, 240
    send(4, 0, u(7)); send(6, 2, u(8, 7)); send(8, 1, u(9))
    send(9, 2, string('bench')); send(9, 3, string('org.lindroids.Bench')); send(7, 6)
    states = []
    def handle(o, op, b):
        if o == 6 and op == 0: send(6, 3, b)
        if o == 8 and op == 0: send(8, 4, b)
        if o == 9 and op == 0:
            n = struct.unpack_from('=I', b, 8)[0]
            states.append(list(struct.unpack_from('=' + 'I' * (n // 4), b, 12)))
    while True:
        o, op, b = event(); handle(o, op, b)
        if o == 8 and op == 0: break
    fd = os.memfd_create('bench'); os.ftruncate(fd, W * H * 4 * 2)
    mm = mmap.mmap(fd, W * H * 4 * 2)
    send(5, 0, u(10, W * H * 4 * 2), fd)
    send(10, 0, u(11, 0, W, H, W * 4, 0)); send(10, 0, u(12, W * H * 4, W, H, W * 4, 0))
    nid = 13; frames = 0; t0 = time.monotonic(); last = t0
    def draw(k):
        nonlocal nid, frames
        off = (k % 2) * W * H * 4
        mm[off:off + W * H * 4] = bytes([k % 256, 0x40, 0x80, 0xff]) * (W * H)
        cb = nid; nid += 1
        send(7, 3, u(cb))  # frame
        send(7, 1, u(11 + k % 2, 0, 0)); send(7, 2, u(0, 0, W, H)); send(7, 6)
        frames += 1
        return cb
    cb = draw(0)
    sock.settimeout(0.2)
    log = []
    while True:
        try:
            o, op, b = event(); handle(o, op, b)
            if o == cb and op == 0 and mode == 'anim':
                cb = draw(frames)
        except socket.timeout:
            pass
        now = time.monotonic()
        if now - last >= 1:
            log.append(frames); last = now
            json.dump({'frames': log, 'states': states}, open(out, 'w'))


def switches(pid):
    total = 0
    for t in Path(f'/proc/{pid}/task').iterdir():
        try:
            total += int((t / 'status').read_text().split('voluntary_ctxt_switches:')[1].split()[0])
        except (OSError, IndexError):
            pass
    return total


def wakeups(pid, secs):
    a = switches(pid)
    time.sleep(secs)
    return (switches(pid) - a) / secs


def main(binary):
    binary = str(Path(binary).resolve())
    # Short: a Unix socket's path has a length limit.
    work = Path(tempfile.mkdtemp(prefix='lw-'))
    rt = work / 'rt'
    rt.mkdir()
    env = dict(os.environ, LINDROIDS_RUNTIME_DIR=str(rt), LINDROIDS_STATE_DIR=str(work / 'state'), LINDROIDS_X11='off',
               LINDROIDS_BROWSER='off', XDG_RUNTIME_DIR=str(rt))
    env.pop('LD_PRELOAD', None)
    name = 'lindroids.wakeups.' + str(os.getpid())
    procs = []

    def start(args, log, extra=None):
        p = subprocess.Popen(args, env=env | (extra or {}), stdout=open(work / log, 'w'), stderr=subprocess.STDOUT)
        procs.append(p)
        return p

    def until(what, predicate, secs=10):
        t0 = time.monotonic()
        while not predicate():
            assert time.monotonic() - t0 < secs, 'timed out: ' + what
            time.sleep(0.05)

    def state(path):
        try:
            return json.loads(path.read_text())
        except (OSError, ValueError):
            return None

    def link(log):
        p = start([binary, 'debug-app', '--socket-name', name, '--dir', str(work / log)], log + '.log')
        until('page linked', lambda: 'linked' in (work / (log + '.log')).read_text())
        return p

    ok = False
    try:
        server = start([binary, 'server', '--runtime-dir', str(rt), '--socket-name', name], 'server.log')
        until('server', lambda: (rt / 'lindroids.sock').exists())
        app = link('frames')
        wl = next(p.name for p in rt.iterdir() if p.name.startswith('wayland-') and not p.name.endswith('.lock'))
        me = [sys.executable, os.path.abspath(__file__), '--client']
        start(me + ['idle', str(work / 'idle.json')], 'idle.log', {'WAYLAND_DISPLAY': wl})
        until('idle window drawn', lambda: list((work / 'frames').glob('window-*.png')))
        time.sleep(3)
        w = wakeups(server.pid, 10)
        assert w < 1.0, f'idle server woke {w:.2f} times a second'
        print(f'PASS idle with a page and a window: {w:.2f} wakeups/s')

        start(me + ['anim', str(work / 'anim.json')], 'anim.log', {'WAYLAND_DISPLAY': wl})
        until('animating', lambda: (state(work / 'anim.json') or {}).get('frames', [0])[-1] > 5)
        app.kill()
        app.wait()
        time.sleep(3)
        a = state(work / 'anim.json')['frames'][-1]
        time.sleep(10)
        fps = (state(work / 'anim.json')['frames'][-1] - a) / 10
        assert fps <= 2, f'a program with no page drew {fps:.1f} frames a second'
        told = state(work / 'idle.json')['states']
        assert 9 in told[-1], f'not told it is suspended: {told}'
        print(f'PASS page gone: the program draws {fps:.1f} frames a second and is suspended')

        link('again')
        until('frame for the page that linked again', lambda: list((work / 'again').glob('window-*.png')), 5)
        until('no longer suspended', lambda: 9 not in state(work / 'idle.json')['states'][-1], 5)
        print('PASS page back: a frame at once, the program no longer suspended')
        ok = True
    finally:
        for p in procs:
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
        time.sleep(0.5)
        for p in procs:
            if p.poll() is None:
                p.kill()
        if not ok:
            print('SERVER LOG:', (work / 'server.log').read_text()[-4000:], file=sys.stderr)


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--client':
        client(sys.argv[2], sys.argv[3])
    else:
        main(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lindroids')
