"""Execution budgets survive process transport without changing their values."""
import importlib.util
import os
from pathlib import Path
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    'gui_policy_contract', Path(__file__).resolve().parents[2] / 'tests/gui/support/policy.py')
policy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(policy)


class GuiPolicyContracts(unittest.TestCase):
    def test_fractional_durations_and_stall_limit_survive_process_transport(self):
        expected = policy.Policy().overrides(run=2.5, call=.75, stall=.5,
                                            cleanup_receipt=.5, consecutive_stalls=3)
        with patch.dict(os.environ, {policy.ENVIRONMENT: expected.dump()}):
            self.assertEqual(policy.Policy.load(), expected)


if __name__ == '__main__':
    unittest.main()
