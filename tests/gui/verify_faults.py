"""Verify runner failure handling and cleanup on an explicitly selected SSH target."""
import argparse
import json
from pathlib import Path
import xml.etree.ElementTree as ET


FAULT_MODES = ('assertion', 'driver-crash', 'application-crash', 'driver-hang', 'application-hang')


def verify(output, mode, exit_code):
    result = json.loads((output / 'result.json').read_text(encoding='utf-8'))
    receipt = json.loads((output / 'test_execution_fault_contract' / 'injected-fault.json')
                         .read_text(encoding='utf-8'))
    assert receipt['mode'] == mode, receipt
    expected = 37 if mode == 'driver-crash' else 1
    assert exit_code == result['exit_code'] == expected, result
    assert not result['timed_out'] and not result.get('error') and not result.get('cancelled'), result
    cleanup = result['cleanup']
    assert cleanup['pytest_exited'] and cleanup['applications_exited'] and cleanup['configs_removed'], result
    assert cleanup['applications_checked'] == 1 and not cleanup['errors'], result
    if mode.endswith('-hang'):
        assert result['elapsed_seconds'] >= 3, result
    elif mode != 'driver-crash':
        suite = ET.parse(output / 'junit.xml').getroot().find('testsuite')
        assert suite.attrib['tests'] == '1' and suite.attrib['errors'] == '0', suite.attrib
        failure = suite.find('testcase/failure')
        reason = ('deliberate infrastructure assertion' if mode == 'assertion'
                  else 'Application unexpectedly exited')
        assert failure is not None and reason in failure.get('message', ''), ET.tostring(suite)
    return result


def main():
    import run
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=Path, help='SSH target JSON')
    parser.add_argument('--exe', required=True, type=Path, help='Debug GUI executable')
    parser.add_argument('--winmd', required=True, type=Path, help='Metadata input')
    parser.add_argument('--output', required=True, type=Path, help='Absent or empty evidence directory')
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        parser.error('--output must be absent or empty')
    output.mkdir(parents=True, exist_ok=True)
    results = {}
    for mode in FAULT_MODES:
        print('Checking ' + mode, flush=True)
        evidence = output / mode
        code = run.main(['--config', str(args.config), '--exe', str(args.exe),
                         '--winmd', str(args.winmd), '--output', str(evidence),
                         '--', '--fault-mode', mode, '--timeout=3'])
        results[mode] = verify(evidence, mode, code)
    (output / 'verification.json').write_text(json.dumps(results, indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
