"""Real operator-calculation gates inside the owned CSI fixture; stdlib only."""
from fractions import Fraction
import json
from pathlib import Path
import subprocess


def query(binary, config, namespace, *args, output='json', units='GiB'):
    command = [str(binary), '--kubeconfig', str(config), '-n', namespace,
               '--view', 'stats', '--timeout', '20s', '--units', units, *args, '-o', output]
    result = subprocess.run(command, text=True, capture_output=True, timeout=30)
    if result.returncode != 0:
        raise RuntimeError(f'calculation query failed with exit {result.returncode}')
    return json.loads(result.stdout) if output == 'json' else result.stdout


def validate_growth(report, before, timestamp_ns):
    """Check against actual raw before/current bytes, independently of Rust math."""
    old = {record['id']: record for record in before['records']}
    current = report['records']
    rates = report['calculations']['comparison']['records']
    if len(current) != 1 or len(rates) != 1 or not report['selection']['complete']:
        raise RuntimeError('CSI calculation did not retain its single shared claim')
    record, rate = current[0], rates[0]
    previous = old[record['id']]
    elapsed = timestamp_ns(record['filesystem']['observed_at']) - timestamp_ns(previous['filesystem']['observed_at'])
    delta = int(record['filesystem']['used_bytes']) - int(previous['filesystem']['used_bytes'])
    if elapsed <= 0 or delta <= 0 or rate['status'] != 'growing':
        raise RuntimeError('CSI growth gate requires advancing timestamps and increased used bytes')
    actual = rate['growth_bytes_per_second']
    expected = Fraction(delta * 10**9, elapsed)
    if Fraction(int(actual['numerator']), int(actual['denominator'])) != expected:
        raise RuntimeError('CSI growth rate differs from actual sample interval and byte delta')
    full = rate['time_to_full_seconds']
    if Fraction(int(full['numerator']), int(full['denominator'])) != Fraction(int(record['filesystem']['available_bytes'])) / expected:
        raise RuntimeError('CSI time to full did not use actual available bytes and observed growth')
    root = report['calculations']['rows'][0]
    if root['statistics']['requested']['sum_bytes'] != str(1024**3) or root['growth']['complete'] is not True:
        raise RuntimeError('CSI calculation multiplied shared capacity or omitted a rate pair')
    return {'status': 'passed', 'record_id': record['id'], 'delta_used_bytes': str(delta),
            'interval_nanoseconds': str(elapsed), 'growth_bytes_per_second': actual,
            'time_to_full_seconds': full, 'physical_total': False}


def verify_csi(kubectl, config, binary, evidence, run_id, namespace, image, timestamp_ns, query_config=None):
    """Reuse the CSI write, then validate a real StatefulSet sharing that claim."""
    evidence = Path(evidence)
    before_path = evidence / 'csi-before-report.json'
    before = json.loads(before_path.read_text())
    report = query(binary, query_config or config, namespace, '--pvc', 'data', '--compare', str(before_path), '--require-metrics')
    growth = validate_growth(report, before, timestamp_ns)
    (evidence / 'calculations-growth.json').write_text(json.dumps(report, indent=2) + '\n')
    for output in ('text', 'csv'):
        result=query(binary,query_config or config,namespace,'--pvc','data','--compare',str(before_path),'--require-metrics',output=output)
        (evidence / ('calculations-growth.' + ('txt' if output=='text' else 'csv'))).write_text(result)
    for units in ('MB','GB','TB'):
        result=query(binary,query_config or config,namespace,'--pvc','data',output='text',units=units)
        (evidence / f'calculations-{units.lower()}.txt').write_text(result)
    name = 'calculation-db-' + run_id[:8]
    fixture = {'apiVersion': 'apps/v1', 'kind': 'StatefulSet', 'metadata': {'name': name, 'namespace': namespace},
               'spec': {'replicas': 2, 'serviceName': name, 'selector': {'matchLabels': {'app': name}},
                        'template': {'metadata': {'labels': {'app': name}}, 'spec': {
                            'terminationGracePeriodSeconds': 1,
                            'containers': [{'name': 'probe', 'image': image, 'command': ['sleep', '3600'],
                                            'resources': {'requests': {'cpu': '5m', 'memory': '8Mi'}, 'limits': {'cpu': '100m', 'memory': '32Mi'}},
                                            'volumeMounts': [{'name': 'data', 'mountPath': '/data', 'readOnly': True}]}],
                            'volumes': [{'name': 'data', 'persistentVolumeClaim': {'claimName': 'data'}}]}}}}
    path = evidence / 'calculations-statefulset-fixture.json'
    path.write_text(json.dumps(fixture, indent=2) + '\n')
    created = False
    try:
        kubectl(config, 'create', '-f', str(path))
        created = True
        kubectl(config, '-n', namespace, 'rollout', 'status', 'statefulset/' + name, '--timeout=90s')
        owner = json.loads(kubectl(config, '-n', namespace, 'get', 'statefulset', name, '-o', 'json'))['metadata']['uid']
        data = query(binary, query_config or config, namespace, '--statefulset', name, '-g', 'statefulset,accessmode,volumemode')
        root = data['calculations']['rows'][0]
        owned = [pod for pod in data['consumers'] if pod.get('statefulset', {}).get('uid') == owner]
        if len(owned) != 2 or len(data['records']) != 1 or root['statistics']['requested']['sum_bytes'] != str(1024**3):
            raise RuntimeError('StatefulSet calculation lost controller identity or duplicated its shared claim')
        if data['groups'][0]['value'] != f'{namespace}/{name}@{owner}':
            raise RuntimeError('StatefulSet grouping did not retain the controller UID')
        (evidence / 'calculations-statefulset.json').write_text(json.dumps(data, indent=2) + '\n')
        for output in ('text', 'csv'):
            suffix = 'txt' if output == 'text' else 'csv'
            result = query(binary, query_config or config, namespace, '--statefulset', name, output=output)
            (evidence / f'calculations-statefulset.{suffix}').write_text(result)
        return {'status': 'passed', 'growth': growth, 'statefulset': {'name': name, 'uid': owner, 'pod_count': len(owned), 'claim_count': 1}}
    finally:
        if created:
            kubectl(config, '-n', namespace, 'delete', 'statefulset', name, '--cascade=foreground', '--wait=true', '--timeout=60s')
