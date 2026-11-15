"""Append one visible result only after all pytest case phases have finished."""
from pathlib import Path


class CaseProgress:
    def __init__(self, output):
        self.path = Path(output) / 'progress.txt'
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.outcomes = {}

    def pytest_runtest_logreport(self, report):
        outcomes = self.outcomes.setdefault(report.nodeid, [])
        outcomes.append(report.outcome)
        if report.when == 'teardown':
            marker = 'F' if 'failed' in outcomes else 'S' if 'skipped' in outcomes else '.'
            with self.path.open('ab') as stream:
                stream.write(marker.encode('ascii'))
            del self.outcomes[report.nodeid]
