#!/usr/bin/env python3
"""Select expensive checks from changed paths; unknown paths conservatively run Rust checks."""
import argparse
import json
import os
from pathlib import Path
import subprocess


CI_TOOL_ONLY = frozenset({
    '.github/actions/cargo-tool/action.yml',
    '.github/workflows/maintenance.yml',
    '.github/workflows/fuzz.yml',
    'scripts/coverage.sh',
    'scripts/check-coverage.py',
    'scripts/fuzz-smoke.sh',
})


def rust_needed(paths):
    for name in paths:
        # Contract examples are compiled into tests; live captures are docs.
        if name.startswith('examples/live/') or name.endswith('.md') or name == 'LICENSE':
            continue
        if name in ('scripts/test-credential-terminal.py', 'scripts/test-smoke-http.py'):
            return True
        if name.startswith(('docs/', 'scripts/test-', 'dev/fixtures/', 'dev/csi/', 'dev/kind/')):
            if name == 'docs/schema/report.schema.json':
                return True
            continue
        if name == '.github/dependabot.yml':
            continue
        return True
    return False


def distribution_needed(paths):
    # These inputs affect validation tools, not the compiled plugin, its source
    # package or Krew archives. Mixed/unknown/build inputs still select releases.
    return rust_needed([name for name in paths if name not in CI_TOOL_ONLY])


def verified_push_base(after):
    repository = os.environ.get('GITHUB_REPOSITORY')
    if not repository:
        return None
    try:
        result = subprocess.run(['gh', 'api', f'repos/{repository}/actions/workflows/ci.yml/runs?branch=main&status=success&event=push&per_page=30'], check=True, capture_output=True, text=True, timeout=30)
        for run in json.loads(result.stdout)['workflow_runs']:
            candidate = run['head_sha']
            if subprocess.run(['git', 'merge-base', '--is-ancestor', candidate, after], capture_output=True, timeout=30).returncode == 0:
                return candidate
    except (subprocess.SubprocessError, KeyError, ValueError):
        pass
    return None


def changed_paths(event, event_name):
    if event_name == 'workflow_dispatch':
        return None
    if event_name == 'pull_request':
        before = event['pull_request']['base']['sha']
        after = event['pull_request']['head']['sha']
        revision = before + '...' + after
    elif event_name == 'push':
        after = event['after']
        before = verified_push_base(after)
        if before is None:
            return None
        # Include changes from cancelled/failed pushes. A following docs-only
        # commit must not turn unverified source into a passing skipped build.
        revision = before + '..' + after
    else:
        return None
    # Missing history, force pushes or unsupported events must never silently
    # skip a required check. The caller checks out full history.
    try:
        result = subprocess.run(['git', 'diff', '--name-only', '-z', revision], check=True, capture_output=True, timeout=30)
    except subprocess.SubprocessError:
        return None
    return [name for name in result.stdout.decode().split('\0') if name]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--event', type=Path, default=os.environ.get('GITHUB_EVENT_PATH'))
    parser.add_argument('--event-name', default=os.environ.get('GITHUB_EVENT_NAME', 'workflow_dispatch'))
    args = parser.parse_args()
    event = json.loads(args.event.read_text()) if args.event else {}
    paths = changed_paths(event, args.event_name)
    needed = paths is None or rust_needed(paths)
    distribution = paths is None or distribution_needed(paths)
    lines = f'rust={str(needed).lower()}\ndistribution={str(distribution).lower()}\n'
    if output := os.environ.get('GITHUB_OUTPUT'):
        with open(output, 'a') as stream:
            stream.write(lines)
    print(lines, end='')
    if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
        with open(summary, 'a') as stream:
            stream.write(f'### Check selection\n\nRust checks: **{needed}**. Release checks: **{distribution}**. ' + ('Full run: manual/initial event or history unavailable.\n' if paths is None else f'{len(paths)} changed paths examined; documentation checks always run.\n'))


if __name__ == '__main__':
    main()
