"""Run GUI behavior scenarios with bounded, verified resource recovery."""
import argparse
from pathlib import Path
import sys
import uuid

from support import execution, progress
from support.policy import Policy
from support.results import succeeded


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--desktop', choices=('isolated', 'current'), default='isolated')
    parser.add_argument('--timeout', type=float)
    parser.add_argument('--case-timeout', type=float)
    parser.add_argument('--stall-after', type=float)
    parser.add_argument('--consecutive-stalls', type=int)
    parser.add_argument('--output', type=Path)
    args, tests = parser.parse_known_args()
    if '--real-input' in tests and args.desktop != 'current':
        parser.error('--real-input requires --desktop current')
    policy = Policy.load().overrides(run=args.timeout, call=args.case_timeout,
                                    stall=args.stall_after, consecutive_stalls=args.consecutive_stalls)
    if min(policy.run, policy.call, policy.stall, policy.consecutive_stalls) <= 0:
        parser.error('timeout budgets must be positive')
    output = (args.output or Path('tmp/gui-tests') / uuid.uuid4().hex).resolve()
    previous = {'key': None, 'at': 0}

    def slow(folder, now):
        active = progress.active(folder)
        if not active or not active.get('active') or active['elapsed'] < policy.slow_case:
            return
        key = (active['nodeid'], active['phase'], active.get('operation'))
        if previous['key'] == key and now - previous['at'] < 5:
            return
        previous.update(key=key, at=now)
        print('[slow] %6.1fs %s %s: %s' % (active['elapsed'], active['nodeid'],
              active['phase'], active.get('operation', '')), flush=True)

    try:
        result = execution.execute(output, tests, policy, args.desktop, on_tick=slow)
    except execution.UnconfirmedExit as error:
        print(str(error), file=sys.stderr, flush=True)
        return 1
    except ValueError as error:
        parser.error(str(error))
    if result.get('error'):
        print(result['error'], file=sys.stderr, flush=True)
    timing = progress.summary(output, policy.slow_case)
    if timing['cases']:
        print('GUI: %d cases, %.1fs, %d slow (>=%.0fs)' %
              (timing['cases'], timing['seconds'], timing['slow'], policy.slow_case), flush=True)
        for row in timing['slowest'][:10]:
            print('  %6.1fs %s' % (row.get('duration', 0), row['nodeid']), flush=True)
    return 0 if succeeded(result) else 1


if __name__ == '__main__':
    raise SystemExit(main())
