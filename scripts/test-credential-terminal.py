#!/usr/bin/env python3
"""Real PTY authentication, foreground restoration and Ctrl-C regression gate."""
import argparse
import errno
import hashlib
from http.server import BaseHTTPRequestHandler
import json
import os
from pathlib import Path
import pty
import select
import signal
import tempfile
import threading
import time
import subprocess
import sys
from urllib.parse import urlsplit
from smoke_http import LoopbackHTTPServer

ROOT = Path(__file__).resolve().parents[1]
TOKEN = "fixture-token-not-a-real-credential"


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if urlsplit(self.path).path != "/api/v1/namespaces/demo/persistentvolumeclaims" or self.headers.get("Authorization") != "Bearer " + TOKEN:
            self.send_error(403)
            return
        self.server.fixture_requests += 1
        body = b'{"metadata":{"resourceVersion":"1"},"items":[]}'
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def run_case(binary, server, mode, interrupt):
    with tempfile.TemporaryDirectory(prefix="view-storage-terminal-") as directory:
        directory = Path(directory)
        marker = directory / "processes"
        script = "printf '%s %s %s' \"$VIEW_STORAGE_INTERNAL_EXEC_PARENT\" \"$PPID\" \"$$\" > \"$1\"; "
        script += "case \"$KUBERNETES_EXEC_INFO\" in *'\"interactive\":true'*) ;; *) exit 8 ;; esac; "
        if interrupt:
            script += "trap '' INT TERM; printf 'AUTH_READY\\n' >&2; exec /bin/sleep 5"
        else:
            script += "printf 'AUTH_INPUT\\n' >&2; IFS= read -r answer; test \"$answer\" = fixture-answer || exit 9; "
            script += "printf '%s' '{\"apiVersion\":\"client.authentication.k8s.io/v1\",\"kind\":\"ExecCredential\",\"status\":{\"token\":\"" + TOKEN + "\"}}'"
        config = directory / "config.json"
        value = {"apiVersion":"v1", "kind":"Config", "clusters":[{"name":"fixture","cluster":{"server":f"http://127.0.0.1:{server.server_port}"}}], "users":[{"name":"fixture","user":{"exec":{"apiVersion":"client.authentication.k8s.io/v1","interactiveMode":mode,"command":"/bin/sh","args":["-c",script,"fixture",str(marker)]}}}], "contexts":[{"name":"fixture","context":{"cluster":"fixture","user":"fixture","namespace":"demo"}}], "current-context":"fixture"}
        config.write_text(json.dumps(value))
        config.chmod(0o600)
        before = server.fixture_requests
        # The shell remains alive after the command and reads again. It can do
        # that only if the original foreground group was restored correctly.
        wrapper = '"$1" --kubeconfig "$2" --timeout 8s -o json; status=$?; printf "\\nCHECK_INPUT_RESTORED\\n"; IFS= read -r answer; [ "$answer" = restored ] || exit 90; exit "$status"'
        master, slave = pty.openpty()
        # Use the standard subprocess spawn path with no Python preexec callback;
        # forking a Python HTTP server's threaded interpreter is unsafe on macOS.
        process = subprocess.Popen([sys.executable,str(Path(__file__).resolve()),"--pty-bootstrap",wrapper,str(binary),str(config)],
            stdin=slave,stdout=slave,stderr=slave,start_new_session=True)
        os.close(slave)
        pid = process.pid
        output = bytearray()
        status = None
        answered = 0
        restored = sent_interrupt = False
        started = time.monotonic()
        try:
            until = started + 5
            while time.monotonic() < until:
                readable, _, _ = select.select([master], [], [], 0.02)
                if readable:
                    try:
                        data = os.read(master, 64 * 1024)
                    except OSError as error:
                        if error.errno != errno.EIO:
                            raise
                        data = b""
                    output.extend(data)
                # The SDK may acquire exec credentials separately for TLS
                # identity and the request auth layer. Service every actual
                # prompt while checking the same terminal ownership contract.
                while answered < output.count(b"AUTH_INPUT"):
                    os.write(master, b"fixture-answer\n")
                    answered += 1
                if interrupt and not sent_interrupt and b"AUTH_READY" in output:
                    # Exercise terminal-generated SIGINT to the foreground
                    # SDK worker group, including relay back to its supervisor.
                    os.write(master, b"\x03")
                    sent_interrupt = True
                if not restored and b"CHECK_INPUT_RESTORED" in output:
                    os.write(master, b"restored\n")
                    restored = True
                status = process.poll()
                if status is not None:
                    break
            if status is None:
                details = {"answered":answered,"restored":restored,"interrupt":sent_interrupt,"bytes":len(output),"http_requests":server.fixture_requests-before,"foreground_group":os.tcgetpgrp(master)}
                if marker.is_file():
                    details["owned_processes"] = [int(value) for value in marker.read_text().split()]
                raise RuntimeError("PTY authentication/foreground watchdog: " + json.dumps(details))
            expected = 130 if interrupt else 0
            if status != expected or not restored:
                raise RuntimeError("PTY authentication exit/foreground restoration mismatch")
            if interrupt:
                if not sent_interrupt or server.fixture_requests != before:
                    raise RuntimeError("PTY cancellation failed to stop before collection")
            else:
                text = output.decode()
                document, _ = json.JSONDecoder().raw_decode(text[text.index("{"):])
                if document["selection"]["complete"] is not True or server.fixture_requests != before + 1:
                    raise RuntimeError("PTY helper did not authenticate the real inventory query")
            if TOKEN.encode() in output:
                raise RuntimeError("PTY output exposed the fixture credential")
            identities = [int(value) for value in marker.read_text().split()]
            until = time.monotonic() + 2
            while time.monotonic() < until:
                remaining = []
                for identity in identities:
                    try:
                        os.kill(identity, 0)
                        remaining.append(identity)
                    except ProcessLookupError:
                        pass
                if not remaining:
                    break
                time.sleep(0.01)
            if remaining:
                raise RuntimeError("PTY gate left an owned helper/root process")
            return {"mode":mode, "interrupt":interrupt, "exit":expected, "foreground_restored":True, "helpers_reaped":True, "input_prompts":answered, "seconds":time.monotonic()-started}
        finally:
            if status is None:
                # Marker identities are private to this fixture; the original
                # shell remains waitable while its child groups are cleaned.
                if marker.is_file():
                    identities = [int(value) for value in marker.read_text().split()]
                    if len(identities) == 3:
                        try:
                            os.killpg(identities[1], signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                        except PermissionError:
                            # macOS reports EPERM for an exited-only group.
                            # This is emergency cleanup of the fixture's group.
                            pass
                try:
                    os.killpg(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
            os.close(master)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/kubectl-view_storage")
    parser.add_argument("--evidence-directory", type=Path, default=ROOT / "artifacts/authentication")
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error("build the real command first")
    destination = args.evidence_directory
    destination.mkdir(parents=True,exist_ok=True)
    result_path = destination / "terminal-validation.json"
    receipt = {"status":"running","binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),"cases":[],"scope":"real PTY, configured SDK helper and authorized mock HTTP; fake credentials only"}
    result_path.write_text(json.dumps(receipt,indent=2)+"\n")
    print("CHECK credential terminal loopback fixture", flush=True)
    server = LoopbackHTTPServer(Handler)
    server.fixture_requests = 0
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        for mode in ("Always","IfAvailable"):
            print("CHECK terminal authentication: " + mode, flush=True)
            receipt["cases"].append(run_case(binary,server,mode,False))
        print("CHECK terminal Ctrl-C and helper cleanup", flush=True)
        receipt["cases"].append(run_case(binary,server,"Always",True))
        receipt["status"] = "passed"
    except BaseException as error:
        receipt.update(status="failed",error_kind=type(error).__name__)
        raise
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
        result_path.write_text(json.dumps(receipt,indent=2)+"\n")
    print("PASS terminal Always/IfAvailable authentication, real Ctrl-C, foreground restoration and helper cleanup (3 cases)")


if __name__ == "__main__":
    if len(sys.argv)>1 and sys.argv[1]=="--pty-bootstrap":
        import fcntl
        import termios
        fcntl.ioctl(0,termios.TIOCSCTTY,0)
        os.tcsetpgrp(0,os.getpgrp())
        os.execv("/bin/sh",["sh","-c",sys.argv[2],"fixture",sys.argv[3],sys.argv[4]])
    else:
        main()
