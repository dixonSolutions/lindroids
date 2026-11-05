#!/usr/bin/env python3
"""Run-level launcher/portal regressions, no Android or extra Python packages required.

cargo build -p lindroids
python3 tests/run-regressions.py [target/debug/lindroids]
LINDROIDS_TEST_TLS=1 also checks public HTTPS using curl and the merged CA exports.
LINDROIDS_TEST_ELECTRON=/path/to/electron also checks Electron's CA loader and picker.
The debug app stands in for the Android page; fake root reproduces Cortex's GIO failure.
"""
import array
import json
import os
from pathlib import Path
import signal
import shutil
import pty
import socket
import struct
import subprocess
import sys
import tempfile
import time


def window_child(close):
    # A tiny Wayland client, directly on the wire: a real, mapped xdg_toplevel with a
    # shared-memory buffer. Delayed until after its launcher has exited and been reaped.
    time.sleep(0.5)
    sock = socket.socket(socket.AF_UNIX)
    sock.settimeout(3)
    sock.connect(str(Path(os.environ['XDG_RUNTIME_DIR']) / os.environ['WAYLAND_DISPLAY']))
    u = lambda *xs: struct.pack('=' + 'I' * len(xs), *xs)

    def string(text):
        data = text.encode() + b'\0'
        return u(len(data)) + data + b'\0' * (-len(data) % 4)

    def send(obj, op, body=b'', fd=None):
        data = u(obj, ((len(body) + 8) << 16) | op) + body
        if fd is None:
            sock.sendall(data)
        else:
            sock.sendmsg([data], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array('i', [fd]))])

    buf = b''

    def event():
        nonlocal buf
        while len(buf) < 8 or len(buf) < struct.unpack_from('=I', buf, 4)[0] >> 16:
            data = sock.recv(65536)
            if not data:
                raise RuntimeError('Wayland disconnected')
            buf += data
        obj, header = struct.unpack_from('=II', buf)
        size = header >> 16
        body, buf = buf[8:size], buf[size:]
        if obj == 1 and header & 65535 == 0:
            raise RuntimeError('Wayland error: ' + repr(body))
        return obj, header & 65535, body

    send(1, 1, u(2))  # get_registry
    send(1, 0, u(3))  # sync
    globals_ = {}
    while True:
        obj, op, body = event()
        if obj == 3:
            break
        if obj == 2 and op == 0:
            name, length = struct.unpack_from('=II', body)
            iface = body[8:8 + length - 1].decode()
            globals_[iface] = name
    for iface, ver, ident in [('wl_compositor', 4, 4), ('wl_shm', 1, 5), ('xdg_wm_base', 1, 6)]:
        send(2, 0, u(globals_[iface]) + string(iface) + u(ver, ident))
    send(4, 0, u(7))  # create_surface
    send(6, 2, u(8, 7))  # get_xdg_surface
    send(8, 1, u(9))  # get_toplevel
    send(9, 2, string('Detached test window'))
    send(9, 3, string('org.lindroids.DetachedTest'))
    send(7, 6)  # initial commit
    while True:
        obj, op, body = event()
        if obj == 6 and op == 0:
            send(6, 3, body)
        if obj == 8 and op == 0:
            send(8, 4, body)  # ack_configure
            break
    fd = os.memfd_create('lindroids-regression')
    os.write(fd, b'\x80\x40\xff\xff' * (64 * 64))
    send(5, 0, u(10, 64 * 64 * 4), fd)
    send(10, 0, u(11, 0, 64, 64, 256, 0))  # ARGB8888 buffer
    send(7, 1, u(11, 0, 0))
    send(7, 2, u(0, 0, 64, 64))
    send(7, 6)
    os.close(fd)
    sock.settimeout(0.1)
    until = time.monotonic() + 30
    while not close.exists() and time.monotonic() < until:
        try:
            obj, op, body = event()
            if obj == 6 and op == 0:
                send(6, 3, body)
        except socket.timeout:
            pass
    sock.close()


def check_suite(binary):
    binary = Path(binary).resolve()
    script = Path(__file__).resolve()
    with tempfile.TemporaryDirectory(prefix='lindroids-regression-') as work:
        work = Path(work)
        rt = work / 'rt'
        rt.mkdir()
        env = dict(os.environ, LINDROIDS_RUNTIME_DIR=str(rt), LINDROIDS_STATE_DIR=str(work / 'state'),
                   LINDROIDS_X11='off', LINDROIDS_BROWSER='off')
        # Keep the test independent of the desktop/session bus and any developer preload.
        env.pop('LD_PRELOAD', None)
        env.pop('SSL_CERT_DIR', None)
        env.pop('SSL_CERT_FILE', None)
        children = []
        log = open(work / 'server.log', 'w+')
        app_log = open(work / 'app.log', 'w+')

        def request(**body):
            with socket.socket(socket.AF_UNIX) as s:
                s.settimeout(3)
                s.connect(str(rt / 'lindroids.sock'))
                s.sendall(json.dumps(body).encode() + b'\n')
                return json.loads(s.makefile('rb').readline())

        def until(what, predicate, seconds=10):
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                try:
                    result = predicate()
                    if result:
                        return result
                except (OSError, ValueError):
                    pass
                time.sleep(0.05)
            raise AssertionError('timed out: ' + what)

        def run(*args, extra=None, timeout=15):
            return subprocess.run([str(binary), 'run', *args], env=env | (extra or {}),
                                  text=True, capture_output=True, timeout=timeout)

        def checked(result):
            assert result.returncode == 0, (result.returncode, result.stdout, result.stderr)
            return result.stdout

        try:
            name = 'lindroids.regression.' + str(os.getpid())
            server = subprocess.Popen([str(binary), 'server', '--runtime-dir', str(rt), '--socket-name', name],
                                      env=env, stdout=log, stderr=log)
            children.append(server)
            until('server', lambda: request(t='status'))
            (work / 'selected.txt').write_text('picked by the debug app\n')
            app = subprocess.Popen([str(binary), 'debug-app', '--socket-name', name, '--dir', str(work / 'frames')],
                                   env=env | {'LINDROIDS_DEBUG_PICK': str(work / 'selected.txt')},
                                   stdin=subprocess.PIPE, stdout=app_log, stderr=app_log)
            children.append(app)
            until('app linked', lambda: 'linked' in (work / 'app.log').read_text())
            close = work / 'close'
            with open(work / 'run.log', 'w+') as run_log:
                gui = subprocess.Popen([str(binary), 'run', sys.executable, str(script), '--launcher', str(close)],
                                       env=env, stdout=run_log, stderr=run_log)
                children.append(gui)
                instances = until('launcher registered', lambda: request(t='instances', app_key='')['instances'])
                pid = instances[0]['pid']
                windows = until('descendant window', lambda: request(t='run_state', pid=pid)['windows'])
                assert windows[0]['app_id'] == 'org.lindroids.DetachedTest', windows
                until('adoption reported', lambda: 'attached window' in (work / 'run.log').read_text())
                assert gui.poll() is None
                assert request(t='run_state', pid=pid)['alive']
                close.touch()
                assert gui.wait(timeout=5) == 0
                assert not request(t='run_state', pid=pid)['alive']
                print('PASS setsid launcher: window attached, app id reported, run ends on window close')
            with open(work / 'signal-gui.log', 'w+') as signal_log:
                gui = subprocess.Popen([str(binary), 'run', sys.executable, str(script), '--launcher',
                                        str(work / 'never-close')], env=env, stdout=signal_log, stderr=signal_log)
                children.append(gui)
                instances = until('second launcher registered', lambda: request(t='instances', app_key='')['instances'])
                pid = instances[0]['pid']
                until('second detached GUI adopted', lambda: 'attached window' in (work / 'signal-gui.log').read_text())
                gui.send_signal(signal.SIGTERM)
                assert gui.wait(timeout=5) == 143
                assert not request(t='run_state', pid=pid)['alive']
                print('PASS termination reaches setsid GUI through the supervisor')
            checked(run('true'))
            assert not request(t='instances', app_key='')['instances']
            print('PASS successful no-window exit: no running state')
            start = time.monotonic()
            checked(run('sh', '-c', 'sleep 8 >/dev/null 2>&1 & exit 0'))
            assert time.monotonic() - start < 7
            assert not request(t='instances', app_key='')['instances']
            print('PASS background helper without window: bounded grace and no stderr EOF hang')
            sleeper = subprocess.Popen([str(binary), 'run', 'sleep', '30'], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            children.append(sleeper)
            until('signal target registered', lambda: request(t='instances', app_key='')['instances'])
            sleeper.send_signal(signal.SIGTERM)
            assert sleeper.wait(timeout=5) == 143
            print('PASS termination signal forwarded to launch group')
            tty_pid, tty_fd = pty.fork()
            if tty_pid == 0:
                os.execve(str(binary), [str(binary), 'run', 'sh', '-c',
                          'read value; test "$value" = terminal-input'], env)
            try:
                os.write(tty_fd, b'terminal-input\n')
                def tty_done():
                    pid, status = os.waitpid(tty_pid, os.WNOHANG)
                    if pid:
                        assert os.waitstatus_to_exitcode(status) == 0, status
                    return pid
                until('interactive terminal input', tty_done, seconds=5)
            finally:
                os.close(tty_fd)
                try:
                    os.kill(tty_pid, signal.SIGKILL)
                    os.waitpid(tty_pid, 0)
                except ProcessLookupError:
                    pass
            print('PASS interactive command retains terminal input')

            dest = ['--session', '--dest', 'org.freedesktop.portal.Desktop', '--object-path',
                    '/org/freedesktop/portal/desktop']
            version = checked(run('gdbus', 'call', *dest, '--method', 'org.freedesktop.DBus.Properties.Get',
                                  'org.freedesktop.portal.FileChooser', 'version'))
            assert 'uint32 4' in version, version
            print('PASS portal version visible from inside a run at startup:', version.strip())
            fake = work / 'fake-root.so'
            src = work / 'fake-root.c'
            src.write_text('unsigned getuid(void){return 0;} unsigned geteuid(void){return 0;} '
                           'unsigned getgid(void){return 0;} unsigned getegid(void){return 0;}')
            subprocess.run(['cc', '-shared', '-fPIC', str(src), '-o', str(fake)], check=True)
            probe = ['gdbus', 'call', *dest, '--method', 'org.freedesktop.DBus.Properties.Get',
                     'org.freedesktop.portal.FileChooser', 'version']
            broken = subprocess.run(probe, env=env | {'LD_PRELOAD': str(fake),
                                    'DBUS_SESSION_BUS_ADDRESS': 'unix:path=' + str(rt / 'bus')},
                                    capture_output=True, text=True, timeout=5)
            assert broken.returncode != 0 and 'credentials' in broken.stderr, broken
            fixed = checked(run(*probe, extra={'LD_PRELOAD': str(fake)}))
            assert 'uint32 4' in fixed, fixed
            checked(run('sh', '-c', 'test "$GTK_USE_PORTAL" = 1', extra={'LD_PRELOAD': str(fake)}))
            checked(run('gdbus', 'call', *dest, '--method', 'org.freedesktop.portal.FileChooser.OpenFile',
                        '', 'Fake root regression', "{'handle_token': <'regression'>}",
                        extra={'LD_PRELOAD': str(fake)}))
            until('picker delivered', lambda: 'pick-files open' in (work / 'app.log').read_text())
            print('PASS fake-root GIO: reproduced EPERM; run repairs credentials and OpenFile reaches picker')
            if os.environ.get('LINDROIDS_TEST_TLS'):
                for host in ['https://example.com', 'https://update.code.visualstudio.com']:
                    tls = checked(run('sh', '-c', 'test -d "$SSL_CERT_DIR" && test -s "$SSL_CERT_FILE" && '
                                      'curl --fail --silent --show-error --max-time 15 -o /dev/null '
                                      '-w "HTTP %{http_code}; TLS verify %{ssl_verify_result}\\n" "$1"', 'tls-test', host,
                                      extra={'SSL_CERT_DIR': '/etc/ssl/certs:/etc/pki/tls/certs'}, timeout=20))
                    print('PASS merged CA TLS', host, tls.strip())
            if os.environ.get('LINDROIDS_TEST_ELECTRON'):
                electron = str(Path(os.environ['LINDROIDS_TEST_ELECTRON']).resolve())
                ca_js = "const tls=require('node:tls'); const fs=require('node:fs'); " \
                        "const certs=tls.getCACertificates('system'); " \
                        "if (!certs.length || process.env.SSL_CERT_DIR.includes(':')) process.exit(1); " \
                        "console.log('Electron '+process.versions.electron+' loaded '+certs.length+' system CAs');"
                print('PASS', checked(run(electron, '-e', ca_js, extra={'ELECTRON_RUN_AS_NODE': '1',
                      'SSL_CERT_DIR': '/etc/ssl/certs:/etc/pki/tls/certs'})).strip())
                app_js = work / 'electron.cjs'
                app_js.write_text("const {app,dialog,net}=require('electron'); "
                    "setTimeout(()=>{console.error('picker timed out');app.exit(2)},15000).unref(); "
                    "app.whenReady().then(async()=>{try {"
                    "if(process.env.LINDROIDS_TEST_TLS){const r=await net.fetch('https://update.code.visualstudio.com');"
                    "console.log('ELECTRON_TLS HTTP '+r.status)}"
                    "const result=await dialog.showOpenDialog({title:'Electron regression',properties:['openFile']});"
                    "console.log('PICKER_RESULT '+JSON.stringify(result)); app.exit(result.canceled?3:0);"
                    "} catch(e) {console.error(e);app.exit(1)}})")
                result = run(electron, '--no-sandbox', '--disable-gpu', '--ozone-platform=wayland',
                             '--xdg-portal-required-version=4', '--user-data-dir=' + str(work / 'electron-data'),
                             str(app_js), extra={'LD_PRELOAD': str(fake),
                             'SSL_CERT_DIR': '/etc/ssl/certs:/etc/pki/tls/certs'}, timeout=20)
                print('PASS Electron portal picker (fake root, required version 4):', checked(result).strip())
                # A high threshold distinguishes a working version override from Chromium's
                # newer on-demand portal path. Disable GTK's portal fallback and our GTK shim.
                high_version = run('--env', 'GTK_USE_PORTAL=0', electron, '--no-sandbox', '--disable-gpu',
                                   '--ozone-platform=wayland', '--xdg-portal-required-version=999',
                                   '--user-data-dir=' + str(work / 'electron-high-version'), str(app_js),
                                   extra={'LD_PRELOAD': str(fake), 'LINDROIDS_GTK_DIALOGS': '1',
                                          'LINDROIDS_TEST_TLS': ''}, timeout=20)
                if high_version.returncode == 0 and 'PICKER_RESULT' in high_version.stdout:
                    print('NOTE Electron portal version override is ignored: required version 999 still used portal 4')
                else:
                    assert high_version.returncode == 2 and 'picker timed out' in high_version.stderr, high_version
                    print('PASS Electron rejects portal 4 when required version is 999')
            # Also exercise ensure_server: infrastructure is a child of this CLI, but must
            # never become part of the application or be killed when that application ends.
            request(t='stop')
            server.wait(timeout=10)
            if app.poll() is None:
                app.terminate()
            app.wait(timeout=5)
            stub_bin = work / 'no-browser-bin'
            stub_bin.mkdir()
            (stub_bin / 'xdg-open').write_text('#!/bin/sh\nexit 1\n')
            (stub_bin / 'xdg-open').chmod(0o755)
            for name in ['dbus-daemon', 'sh']:
                (stub_bin / name).symlink_to(shutil.which(name))
            auto_env = env | {'PATH': str(stub_bin)}
            with open(work / 'auto-run.log', 'w+') as auto_log:
                auto = subprocess.Popen([str(binary), 'run', sys.executable, str(script), '--launcher',
                                         str(work / 'auto-never-close')], env=auto_env,
                                        stdout=auto_log, stderr=auto_log)
                children.append(auto)
                until('automatically started server', lambda: request(t='status'))
                auto_app = subprocess.Popen([str(binary), 'debug-app', '--dir', str(work / 'auto-frames')],
                                            env=auto_env, stdin=subprocess.PIPE, stdout=app_log, stderr=app_log)
                children.append(auto_app)
                until('auto-started GUI adopted', lambda: 'attached window' in (work / 'auto-run.log').read_text())
                auto.send_signal(signal.SIGTERM)
                assert auto.wait(timeout=5) == 143
                assert request(t='status')['t'] == 'status'
                assert not request(t='instances', app_key='')['instances']
                print('PASS auto-started server remains independent of supervised app and survives termination')
        except BaseException:
            print('SERVER LOG:', (work / 'server.log').read_text(), file=sys.stderr)
            print('APP LOG:', (work / 'app.log').read_text(), file=sys.stderr)
            if (work / 'run.log').exists():
                print('RUN LOG:', (work / 'run.log').read_text(), file=sys.stderr)
            raise
        finally:
            try:
                request(t='stop')
            except OSError:
                pass
            for child in reversed(children):
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            log.close()
            app_log.close()


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--launcher':
        if os.fork():
            os._exit(0)
        os.setsid()
        try:
            window_child(Path(sys.argv[2]))
        except BaseException:
            import traceback
            traceback.print_exc()
            os._exit(1)
        os._exit(0)
    else:
        check_suite(sys.argv[1] if len(sys.argv) > 1 else 'target/debug/lindroids')
