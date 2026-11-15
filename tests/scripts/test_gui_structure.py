"""Dependency directions that make GUI scenarios independently collectable."""
import ast
from pathlib import Path
import unittest


GUI = Path(__file__).resolve().parents[2] / 'tests/gui'


class GuiStructure(unittest.TestCase):
    def test_no_scenario_or_fixture_imports_another_test_module(self):
        violations = []
        for path in GUI.rglob('*.py'):
            tree = ast.parse(path.read_text(encoding='utf-8-sig'), filename=str(path))
            for node in ast.walk(tree):
                if isinstance(node, ast.ImportFrom):
                    names = [node.module or '', *(alias.name for alias in node.names)]
                elif isinstance(node, ast.Import):
                    names = [alias.name for alias in node.names]
                else:
                    continue
                if any(part.startswith('test_') for name in names for part in name.split('.')):
                    violations.append('%s:%d' % (path.relative_to(GUI), node.lineno))
        self.assertEqual(violations, [], 'Shared behavior belongs to the driver, not a test provider')

    def test_only_coordinator_publishes_new_terminal_results(self):
        publishers = set()
        for path in GUI.rglob('*.py'):
            tree = ast.parse(path.read_text(encoding='utf-8-sig'))
            for node in ast.walk(tree):
                if not isinstance(node, ast.Call) or not isinstance(node.func, ast.Attribute):
                    continue
                if node.func.attr not in ('write_json', 'write_text'):
                    continue
                target = node.args[0] if node.func.attr == 'write_json' and node.args else node.func.value
                if any(isinstance(value, ast.Constant) and value.value == 'run.json'
                       for value in ast.walk(target)):
                    publishers.add(path.relative_to(GUI).as_posix())
        self.assertEqual(publishers, {'support/execution.py'})


if __name__ == '__main__':
    unittest.main()
