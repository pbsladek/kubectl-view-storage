#!/usr/bin/env python3
"""Run a disposable native Prometheus with TLS and a controlled Kubernetes HTTP fixture.

Requires an explicit Prometheus executable, the built plugin, and OpenSSL.
Never reads a user kubeconfig or starts/stops Docker or an existing server.
"""
import argparse
import contextlib
from fractions import Fraction
import hashlib
from http.server import BaseHTTPRequestHandler
import json
import os
from pathlib import Path
import shutil
import socket
import ssl
import subprocess
import tempfile
import threading
import time
import urllib.parse
import urllib.request

from smoke_http import LoopbackHTTPServer
from dev_csi import timestamp_ns

ROOT = Path(__file__).resolve().parents[1]


def stop_process(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def run(plugin, prometheus, evidence):
    plugin, prometheus, evidence = plugin.resolve(), prometheus.resolve(), evidence.resolve()
    for executable in (plugin, prometheus):
        if not executable.is_file() or not os.access(executable, os.X_OK):
            raise RuntimeError('supply built plugin and Prometheus executables')
    openssl = shutil.which('openssl')
    if not openssl:
        raise RuntimeError('OpenSSL is required for the owned TLS fixture')
    evidence.mkdir(parents=True, exist_ok=True)
    receipt = evidence / 'result.json'
    receipt.write_text(json.dumps({'status': 'running'}) + '\n')
    fixture = json.loads((ROOT / 'examples/contract/healthy/input.json').read_text())['kubernetes']
    owner = {'apiVersion': 'apps/v1', 'kind': 'ReplicaSet', 'name': 'replica', 'uid': 'replica-uid', 'controller': True}
    root = dict(owner, kind='Deployment', name='app', uid='deployment-uid')
    fixture['pods'][0]['metadata']['ownerReferences'] = [owner]
    state = {'scrapes': 0}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            path = urllib.parse.urlsplit(self.path).path
            if path == '/metrics':
                state['scrapes'] += 1
                used = 40 + state['scrapes']
                values = {'capacity_bytes': 1000, 'used_bytes': used, 'available_bytes': 990-used,
                          'inodes': 1000, 'inodes_used': 100, 'inodes_free': 850}
                body = ''.join(f'kubelet_volume_stats_{name}{{namespace="demo",persistentvolumeclaim="data",node="worker-a"}} {value}\n'
                               for name, value in values.items()).encode()
                content = 'text/plain; version=0.0.4'
            else:
                routes = {
                    '/api/v1/namespaces/demo/persistentvolumeclaims': {'metadata': {'resourceVersion': '1'}, 'items': fixture['pvcs']},
                    '/api/v1/namespaces/demo/pods': {'metadata': {'resourceVersion': '1'}, 'items': fixture['pods']},
                    '/api/v1/persistentvolumes/pv-data': fixture['pvs'][0],
                    '/apis/storage.k8s.io/v1/storageclasses/fast': fixture['storage_classes'][0],
                    '/apis/apps/v1/namespaces/demo/replicasets/replica': {'apiVersion': 'apps/v1', 'kind': 'ReplicaSet',
                        'metadata': {'namespace': 'demo', 'name': 'replica', 'uid': 'replica-uid', 'ownerReferences': [root]}, 'spec': {}},
                    '/apis/apps/v1/namespaces/demo/deployments/app': {'apiVersion': 'apps/v1', 'kind': 'Deployment',
                        'metadata': {'namespace': 'demo', 'name': 'app', 'uid': 'deployment-uid'}, 'spec': {}},
                }
                if path not in routes:
                    self.send_error(404)
                    return
                body, content = json.dumps(routes[path]).encode(), 'application/json'
            self.send_response(200)
            self.send_header('Content-Type', content)
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = LoopbackHTTPServer(Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    process = None
    try:
        with contextlib.ExitStack() as cleanup:
            temporary = cleanup.enter_context(tempfile.TemporaryDirectory(prefix='view-storage-prometheus-'))
            owned = Path(temporary)
            def certificate(*args):
                subprocess.run([openssl, *args], cwd=owned, check=True, capture_output=True, timeout=15)
            certificate('req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-days', '1',
                        '-subj', '/CN=view-storage-test-root', '-keyout', 'ca.key', '-out', 'ca.crt',
                        '-addext', 'basicConstraints=critical,CA:TRUE', '-addext', 'keyUsage=critical,keyCertSign,cRLSign',
                        '-addext', 'subjectKeyIdentifier=hash', '-addext', 'authorityKeyIdentifier=keyid:always')
            certificate('req', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes',
                        '-subj', '/CN=localhost', '-keyout', 'server.key', '-out', 'server.csr')
            (owned / 'extensions').write_text('subjectAltName=IP:127.0.0.1,DNS:localhost\nbasicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature\nextendedKeyUsage=serverAuth\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n')
            certificate('x509', '-req', '-in', 'server.csr', '-CA', 'ca.crt', '-CAkey', 'ca.key', '-CAcreateserial',
                        '-days', '1', '-extfile', 'extensions', '-out', 'server.crt')
            with socket.socket() as probe:
                probe.bind(('127.0.0.1', 0))
                port = probe.getsockname()[1]
            api_port = server.server_port
            (owned / 'prometheus.yml').write_text('global:\n  scrape_interval: 1s\nscrape_configs:\n' + ''.join(
                f'  - job_name: {job}\n    static_configs:\n      - targets: ["127.0.0.1:{api_port}"]\n        labels:\n          cluster: fixture\n'
                for job in ('primary', 'replica')))
            (owned / 'web.yml').write_text(f'tls_server_config:\n  cert_file: {owned / "server.crt"}\n  key_file: {owned / "server.key"}\n')
            config = {'apiVersion': 'v1', 'kind': 'Config', 'clusters': [{'name': 'owned', 'cluster': {'server': f'http://127.0.0.1:{api_port}'}}],
                      'users': [{'name': 'owned', 'user': {}}], 'contexts': [{'name': 'owned', 'context': {'cluster': 'owned', 'user': 'owned', 'namespace': 'demo'}}], 'current-context': 'owned'}
            (owned / 'kubeconfig.json').write_text(json.dumps(config))
            with (evidence / 'prometheus.log').open('w') as log:
                process = subprocess.Popen([str(prometheus), '--config.file=' + str(owned / 'prometheus.yml'),
                    '--storage.tsdb.path=' + str(owned / 'data'), '--storage.tsdb.retention.time=15m',
                    f'--web.listen-address=127.0.0.1:{port}', '--web.config.file=' + str(owned / 'web.yml')], stdout=log, stderr=log)
                cleanup.callback(stop_process, process)
                context = ssl.create_default_context(cafile=owned / 'ca.crt')
                url = f'https://127.0.0.1:{port}'
                deadline = time.monotonic() + 30
                last_error = 'no scraped samples'
                while True:
                    if process.poll() is not None:
                        raise RuntimeError('owned Prometheus stopped before readiness; inspect its fixture log')
                    try:
                        with urllib.request.urlopen(url + '/api/v1/query?query=kubelet_volume_stats_used_bytes', context=context, timeout=1) as response:
                            if json.load(response)['data']['result']:
                                break
                    except (OSError, ValueError, KeyError) as error:
                        last_error = str(error)
                    if time.monotonic() >= deadline:
                        raise RuntimeError(f'owned Prometheus did not expose scraped samples before its deadline (scrapes={state["scrapes"]}): {last_error}')
                    time.sleep(0.1)
                command = [str(plugin), '--kubeconfig', str(owned / 'kubeconfig.json'), '--timeout', '5s',
                    '--metrics-source', 'prometheus', '--prometheus-url', url, '--prometheus-cluster', 'cluster=fixture',
                    '--workload', 'deployment/app', '-g', 'workload', '--require-metrics', '-o', 'json']
                untrusted = subprocess.run(command, capture_output=True, text=True, timeout=15)
                if untrusted.returncode != 4 or len(json.loads(untrusted.stdout)['records']) != 1:
                    raise RuntimeError('untrusted TLS must retain inventory and fail required telemetry')
                result = subprocess.run([*command, '--prometheus-ca-file', str(owned / 'ca.crt'), '--view', 'stats',
                    '--watch', '2s', '--watch-count', '3'], capture_output=True, text=True, timeout=30)
                if result.returncode:
                    raise RuntimeError('TLS Prometheus/watch/workload query failed: ' + result.stderr)
                reports = [json.loads(line) for line in result.stdout.splitlines()]
                if len(reports) != 3:
                    raise RuntimeError('watch did not emit three complete reports')
                for index, report in enumerate(reports):
                    record = report['records'][0]
                    if report['groups'][0]['value'] != 'demo/deployment/app@deployment-uid' or report['summary']['record_count'] != 1:
                        raise RuntimeError('verified workload selection lost or duplicated the shared claim')
                    if record['filesystem']['status'] != 'ok' or record['filesystem']['source']['provider'] != 'prometheus':
                        raise RuntimeError('TLS Prometheus did not produce a fresh attributable byte tuple')
                    if report['collection']['cost']['node_requests'] != 0:
                        raise RuntimeError('Prometheus query contacted the kubelet proxy')
                    if report['summary']['filesystem']['used_bytes'] != record['filesystem']['used_bytes']:
                        raise RuntimeError('scrape replicas multiplied filesystem usage')
                    if index:
                        old = reports[index-1]['records'][0]['filesystem']
                        current = record['filesystem']
                        interval = timestamp_ns(current['observed_at']) - timestamp_ns(old['observed_at'])
                        delta = int(current['used_bytes']) - int(old['used_bytes'])
                        rate = report['calculations']['comparison']['records'][0]
                        expected = Fraction(delta * 10**9, interval)
                        actual = rate['growth_bytes_per_second']
                        if delta <= 0 or Fraction(int(actual['numerator']), int(actual['denominator'])) != expected:
                            raise RuntimeError('automatic history differs from actual Prometheus sample intervals and counters')
                    (evidence / f'cycle-{index+1}.json').write_text(json.dumps(report, indent=2) + '\n')
                version = subprocess.check_output([str(prometheus), '--version'], text=True, stderr=subprocess.STDOUT).splitlines()[0]
                receipt.write_text(json.dumps({'status': 'passed', 'prometheus': version,
                    'plugin_sha256': hashlib.sha256(plugin.read_bytes()).hexdigest(), 'cycles': 3,
                    'tls_verification': True, 'untrusted_tls_rejected': True, 'independent_growth_arithmetic': True,
                    'scope': 'native Prometheus with an owned Kubernetes HTTP fixture; not a live Kubernetes/driver claim'}, indent=2) + '\n')
    except BaseException:
        receipt.write_text(json.dumps({'status': 'failed'}) + '\n')
        raise
    finally:
        stop_process(process)
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prometheus-binary', required=True, type=Path)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/kubectl-view_storage')
    parser.add_argument('--evidence-directory', type=Path, default=ROOT / 'artifacts/prometheus-live')
    args = parser.parse_args()
    run(args.binary, args.prometheus_binary, args.evidence_directory)
    print('PASS native Prometheus TLS, scrape deduplication, workload attribution, watch and independent growth arithmetic; owned cleanup complete')
