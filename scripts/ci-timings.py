#!/usr/bin/env python3
"""Report measured Actions job/step time; runner-minutes are not an invoice."""
import argparse
from datetime import datetime
import json
import math
from pathlib import Path
import subprocess


def seconds(start, end):
    if not start or not end:
        return 0.0
    return max(0.0, (datetime.fromisoformat(end.replace('Z', '+00:00')) - datetime.fromisoformat(start.replace('Z', '+00:00'))).total_seconds())


def summarize(run, jobs):
    measured = []
    for job in jobs:
        if job.get('conclusion') == 'skipped' or not job.get('completed_at'):
            continue
        elapsed = seconds(job.get('started_at'), job.get('completed_at'))
        measured.append({
            'name': job['name'], 'url': job['html_url'], 'conclusion': job['conclusion'],
            'seconds': elapsed,
            'queue_seconds': seconds(job.get('created_at'), job.get('started_at')),
            'steps': [{'name': step['name'], 'seconds': seconds(step.get('started_at'), step.get('completed_at')), 'conclusion': step.get('conclusion')}
                      for step in job['steps'] if step.get('completed_at')],
        })
    return {
        'run_id': run['id'], 'revision': run['head_sha'], 'workflow': run['name'], 'event': run['event'],
        'url': run['html_url'], 'status': run['status'], 'conclusion': run.get('conclusion'),
        'runner_minutes': round(sum(job['seconds'] for job in measured)/60, 3),
        'rounded_job_minutes': sum(math.ceil(job['seconds']/60) for job in measured),
        'job_queue_seconds': round(sum(job['queue_seconds'] for job in measured), 3),
        'jobs': measured,
        'scope': 'completed-job wall time, summed across parallel jobs; rounded minutes are an estimate, not billing evidence',
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('run_ids', nargs='+', type=int)
    parser.add_argument('--repository', default='pbsladek/kubectl-view-storage')
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    reports = []
    for run_id in args.run_ids:
        base = f'repos/{args.repository}/actions/runs/{run_id}'
        run = json.loads(subprocess.check_output(['gh', 'api', base], text=True))
        pages = json.loads(subprocess.check_output(['gh', 'api', base+'/jobs?per_page=100', '--paginate', '--slurp'], text=True))
        reports.append(summarize(run, [job for page in pages for job in page['jobs']]))
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(reports, indent=2)+'\n')
    print('RUN          WORKFLOW                          RUNNER MIN  ROUNDED MIN  QUEUED SEC  STATUS')
    for report in reports:
        print(f"{report['run_id']}  {report['workflow']:<32}  {report['runner_minutes']:10.2f}  {report['rounded_job_minutes']:11}  {report['job_queue_seconds']:10.0f}  {report['conclusion'] or report['status']}")
        slowest = sorted(report['jobs'], key=lambda job:job['seconds'], reverse=True)[:3]
        for job in slowest:
            print(f"  {job['seconds']:5.0f}s  {job['name']}")


if __name__ == '__main__':
    main()
