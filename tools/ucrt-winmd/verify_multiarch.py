"""Compare actual Winlex queries against separately generated architecture baselines."""
import csv
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent
EXE = None
REPORT = ROOT / 'verification/multiarch-query.json'
PREVIOUS = None


def normalize(value):
    if isinstance(value, dict):
        return {k: normalize(v) for k, v in value.items()
                if k not in {'id', 'parent_id', 'reference_id', 'architecture_mask'}}
    if isinstance(value, list):
        return [normalize(v) for v in value
                if not (isinstance(v, dict) and v.get('name') == 'SupportedArchitectureAttribute')]
    return value


def query(path, arch, command, name):
    proc = subprocess.run([str(EXE), str(path), command, name, '--arch', arch, '--json'],
                          capture_output=True, text=True, encoding='utf-8')
    data = json.loads(proc.stdout)
    return data, proc.returncode


def check(task):
    arch, command, name = task
    expected, code = query(ROOT / f'out/{arch}/UCRT.winmd', arch, command, name)
    actual, actual_code = query(ROOT / 'out/UCRT.winmd', arch, command, name)
    assert code == 0 and expected['ok'], (task, expected)
    for path in [ROOT / f'out/{arch}/UCRT.untagged.winmd',
                 ROOT / f'out/{arch}/UCRT.roundtrip.winmd',
                 ROOT / 'out/UCRT.roundtrip.winmd']:
        other, other_code = query(path, arch, command, name)
        assert other_code == 0 and other['ok'], (task, str(path), other)
        assert normalize(other['result']) == normalize(expected['result']), (task, str(path), other, expected)
    if PREVIOUS is not None:
        previous, previous_code = query(PREVIOUS / arch / 'UCRT.winmd', arch, command, name)
        assert code == previous_code and expected['ok'] == previous['ok'], (task, previous, expected)
        if expected['ok']:
            before = normalize(previous['result'])
            after = normalize(expected['result'])
            if command == 'show' and name.rsplit('.', 1)[-1] in {'size_t', 'uintptr_t', 'intptr_t', 'ptrdiff_t'}:
                native = 'USize' if name.rsplit('.', 1)[-1] in {'size_t', 'uintptr_t'} else 'ISize'
                allowed = {'U32', 'USize'} if native == 'USize' else {'I32', 'ISize'}
                assert before['fields'][0]['type']['name'] in allowed
                before['fields'][0]['type'] = {'kind': 'primitive', 'name': native}
            assert before == after, (task, before, after)
        else:
            assert expected['error'] == previous['error'], (task, previous, expected)
    assert actual_code == 0 and actual['ok'], (task, actual)
    assert normalize(expected['result']) == normalize(actual['result']), (task, expected['result'], actual['result'])
    if command == 'layout' and name.rsplit('.', 1)[-1] in {'size_t', 'uintptr_t', 'intptr_t', 'ptrdiff_t', 'rsize_t'}:
        bits = 32 if arch == 'x86' else 64
        assert actual['result']['size_bits'] == bits and actual['result']['alignment_bits'] == bits, (task, actual)
    return {'architecture': arch, 'command': command, 'name': name, 'status': 'match'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--winlex', type=Path, required=True, help='Path to a compatible Winlex CLI executable')
    parser.add_argument('--previous', type=Path, help='Optional directory containing pre-normalization architecture baselines')
    options = parser.parse_args()
    EXE = options.winlex.resolve()
    if not EXE.is_file():
        parser.error('Winlex executable not found.')
    PREVIOUS = options.previous
    tasks = []
    methods = {}
    for arch in ['x86', 'x64', 'arm64']:
        with (ROOT / f'out/{arch}/winmd-inventory.tsv').open(encoding='utf-8', newline='') as stream:
            rows = list(csv.DictReader(stream, delimiter='\t'))
        for row in rows:
            if row['kind'] == 'type' and row['name'] != 'Apis':
                for command in ['show', 'layout']:
                    tasks.append((arch, command, 'Ucrt.CRuntime.' + row['name']))
        methods[arch] = {r['name'] for r in rows if r['kind'] == 'method' and r['owner'] == 'Apis'}
        for name in ['printf', 'vprintf', 'malloc', 'qsort', '_beginthreadex', '_stat32', '_stat64',
                     '_findfirst32', '_findfirst64', '_execl', '_time64', '_aligned_malloc']:
            tasks.append((arch, 'show', 'Ucrt.CRuntime.Apis.' + name))
    with ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(check, tasks))
    # Architecture-only functions must not leak into the other query surfaces.
    for arch, names in methods.items():
        for name in sorted(set.union(*methods.values()) - names):
            data, code = query(ROOT / 'out/UCRT.winmd', arch, 'show', 'Ucrt.CRuntime.Apis.' + name)
            assert code != 0 and not data['ok'], (arch, name, data)
            results.append({'architecture': arch, 'command': 'show', 'name': name, 'status': 'correctly-unavailable'})
    REPORT.write_text(json.dumps({'checked_previous_baselines': PREVIOUS is not None, 'checks': results, 'limitations': [
        'baseline equivalence is not independent native ABI validation',
        'function queries are representative; Rust contracts compare all emitted signatures and attributes'
    ]}, indent=2), encoding='utf-8')
    from collections import Counter
    print('Winlex architecture comparisons:', dict(Counter(r['status'] for r in results)))
