#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('ci_timings', Path(__file__).with_name('ci-timings.py'))
timings = importlib.util.module_from_spec(spec)
spec.loader.exec_module(timings)


class TimingTests(unittest.TestCase):
    def test_parallel_job_minutes_and_queue_exclude_dependency_wait(self):
        run = {'id':1,'head_sha':'abc','name':'test','event':'push','html_url':'example','status':'completed','conclusion':'success'}
        def job(name, created, start, end, conclusion='success'):
            return {'name':name,'created_at':f'2026-11-08T00:{created}Z','started_at':f'2026-11-08T00:{start}Z','completed_at':f'2026-11-08T00:{end}Z','conclusion':conclusion,'html_url':'example','steps':[]}
        result = timings.summarize(run,[job('first','00:00','00:01','01:01'),job('parallel','00:00','00:01','01:02'),job('dependent','01:03','01:04','01:09'),job('skipped','00:00','00:00','00:01','skipped')])
        self.assertEqual(result['runner_minutes'],2.1)
        self.assertEqual(result['rounded_job_minutes'],4)
        self.assertEqual(result['job_queue_seconds'],3)
        self.assertEqual(len(result['jobs']),3)

    def test_incomplete_and_clock_inconsistent_samples_are_not_negative(self):
        self.assertEqual(timings.seconds(None,'2026-11-08T00:00:00Z'),0)
        self.assertEqual(timings.seconds('2026-11-08T00:00:01Z','2026-11-08T00:00:00Z'),0)


if __name__ == '__main__':
    unittest.main()
