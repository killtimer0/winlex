"""Single source for run, case and liveness budgets."""
from dataclasses import asdict, dataclass, replace
import json
import os

ENVIRONMENT = 'WINLEX_TEST_POLICY'


@dataclass(frozen=True)
class Policy:
    """Backstops are liveness bounds, not latency assertions."""

    run: float = 900
    startup: float = 15
    setup: float = 30
    call: float = 120
    teardown: float = 30
    collection: float = 60
    stall: float = 20
    slow_case: float = 10
    consecutive_stalls: int = 2
    inventory: float = 10
    cleanup_step: float = 10
    cleanup_receipt: float = 15
    cleanup_wait: float = 10

    def phase(self, name):
        return {'setup': self.setup, 'call': self.call, 'teardown': self.teardown}[name]

    def ledger_budget(self, held, ime):
        """Cleanup ceiling for one owned input ledger."""
        # One release per held key, IME send/wait/overrun, cursor delivery, IO.
        return 5 * (held + 1) + (21 if ime else 0) + 5 + 10

    def overrides(self, **values):
        return replace(self, **{key: value for key, value in values.items()
                                if value is not None})

    def dump(self):
        return json.dumps(asdict(self), sort_keys=True)

    @classmethod
    def load(cls):
        try:
            raw = json.loads(os.environ[ENVIRONMENT])
        except (KeyError, ValueError, TypeError):
            return cls()
        if not isinstance(raw, dict):
            return cls()
        values = {}
        for field, default in asdict(cls()).items():
            value = raw.get(field, default)
            try:
                # Defaults may be written as integer literals, but duration
                # fields are floats. Preserve fractional CLI/probe budgets
                # when transferring the same policy to a child process.
                values[field] = cls.__annotations__[field](value)
            except (TypeError, ValueError):
                values[field] = default
        policy = cls(**values)
        if min(policy.run, policy.startup, policy.setup, policy.call, policy.teardown,
               policy.collection, policy.stall, policy.slow_case, policy.inventory,
               policy.cleanup_step, policy.cleanup_receipt, policy.cleanup_wait) <= 0 or policy.consecutive_stalls <= 0:
            return cls()
        return policy
