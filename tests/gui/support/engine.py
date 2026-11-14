"""The one engine invocation supervised by local and remote runners."""
import sys
from pathlib import Path


def command(output, tests, policy, desktop):
    """`run.py --engine` with the budgets this supervisor already resolved."""
    return [sys.executable, str(Path(__file__).resolve().parent.parent / 'run.py'), '--engine',
            '--output', str(output), '--desktop', desktop,
            '--timeout', str(policy.run), '--case-timeout', str(policy.call),
            '--stall-after', str(policy.stall),
            '--consecutive-stalls', str(policy.consecutive_stalls), *tests]
