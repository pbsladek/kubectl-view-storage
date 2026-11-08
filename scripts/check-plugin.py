#!/usr/bin/env python3
"""Required cluster-free kubectl plugin filename/discovery/forwarding smoke."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib
import threading
from urllib.parse import parse_qs, urlsplit

ROOT = Path(__file__).resolve().parents[1]
EXECUTABLE = "kubectl-view_storage"


def configured_queries(binary,kubectl,directory,env):
    requests = []
    class Handler(BaseHTTPRequestHandler):
        def do_GET(self):
            parsed=urlsplit(self.path)
            requests.append(parsed.path)
            if parsed.path not in ("/api/v1/persistentvolumeclaims","/api/v1/namespaces/cluster/persistentvolumeclaims") or parse_qs(parsed.query).get("limit")!=["500"]:
                self.send_error(400)
                return
            body=b'{"metadata":{"resourceVersion":"1"},"items":[]}'
            self.send_response(200)
            self.send_header("Content-Type","application/json")
            self.send_header("Content-Length",str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self,*_):
            pass
    server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
    thread=threading.Thread(target=server.serve_forever,daemon=True)
    thread.start()
    config=Path(directory)/"fixture.json"
    value={"apiVersion":"v1","kind":"Config","clusters":[{"name":"fixture","cluster":{"server":f"http://127.0.0.1:{server.server_port}"}}],"users":[{"name":"fixture","user":{}}],"contexts":[{"name":"fixture","context":{"cluster":"fixture","user":"fixture","namespace":"demo"}},{"name":"other","context":{"cluster":"fixture","user":"fixture","namespace":"other"}}],"current-context":"fixture"}
    config.write_text(json.dumps(value))
    config.chmod(0o600)
    try:
        for scope in (("-n","cluster"),("-A",)):
            args=["--kubeconfig",str(config),"--context","other",*scope,"-o","json"]
            outputs=[]
            for command in ([str(binary),*args],[kubectl,"view-storage",*args]):
                result=subprocess.run(command,env=env,capture_output=True,text=True,timeout=5)
                if result.returncode or result.stderr:
                    raise RuntimeError("configured direct/plugin query failed")
                document=json.loads(result.stdout)
                if document["collection"]["context"]!="other" or document["selection"]["complete"] is not True or document["collection"]["cost"]["node_requests"]!=0:
                    raise RuntimeError("configured plugin context/selection/cost differs")
                outputs.append({key:document[key] for key in ("query","selection","records","consumers","relationships","groups","summary")})
                if scope[0]=="-n" and document["collection"]["scope"].get("namespace")!="cluster":
                    raise RuntimeError("literal cluster namespace was treated as cluster scope")
            if outputs[0]!=outputs[1]:
                raise RuntimeError("configured direct/plugin semantic reports differ")
        if requests != ["/api/v1/namespaces/cluster/persistentvolumeclaims"]*2+["/api/v1/persistentvolumeclaims"]*2:
            raise RuntimeError("configured plugin did not forward namespace/all-namespace flags")
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def check(binary, kubectl=None):
    manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
    if manifest["package"].get("autobins") is not False or [(target["name"], target["path"]) for target in manifest.get("bin", [])] != [(EXECUTABLE, "src/main.rs")]:
        raise RuntimeError("Cargo must declare the canonical kubectl-view_storage executable")
    binary = Path(binary).absolute()
    if binary.name != EXECUTABLE or not binary.is_file() or not os.access(binary, os.X_OK):
        raise RuntimeError("build the real kubectl-view_storage executable before the plugin smoke")
    kubectl = kubectl or shutil.which("kubectl")
    if not kubectl:
        raise RuntimeError("kubectl is required for the plugin discovery gate")
    kubectl = str(Path(kubectl).resolve())
    cases = [("--help",), ("--version",), ("--kind", "pv"), ("-A", "-n", "demo"), ("--forwarding-probe=sentinel",)]
    with tempfile.TemporaryDirectory(prefix="view-storage-discovery-") as directory:
        (Path(directory) / EXECUTABLE).symlink_to(binary)
        # Isolate discovery from installed plugins and user kubeconfig. All
        # cases finish before configuration/authentication or cluster access.
        env = dict(os.environ, PATH=directory, KUBECONFIG=str(Path(directory) / "absent-config"), NO_COLOR="1")
        for args in cases:
            direct = subprocess.run([str(binary), *args], env=env, capture_output=True, text=True, timeout=5)
            plugin = subprocess.run([kubectl, "view-storage", *args], env=env, capture_output=True, text=True, timeout=5)
            expected = 0 if args[0] in ("--help", "--version") else 2
            if direct.returncode != expected or (plugin.returncode, plugin.stdout, plugin.stderr) != (direct.returncode, direct.stdout, direct.stderr):
                raise RuntimeError("direct/plugin invocation differs for " + args[0])
            if expected == 0 and not direct.stdout or expected == 2 and direct.stdout:
                raise RuntimeError("plugin smoke produced invalid stdout")
        configured_queries(binary,kubectl,directory,env)
    print("PASS canonical filename, kubectl discovery, help/version, forwarding and real configured queries (7 cases)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug" / EXECUTABLE)
    parser.add_argument("--kubectl", type=Path)
    args = parser.parse_args()
    try:
        check(args.binary, args.kubectl)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, f"FAIL plugin discovery: {error}\n")


if __name__ == "__main__":
    main()
