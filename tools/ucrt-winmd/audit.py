"""Independent Clang AST inventory; never uses bnd's selection logic."""
import argparse
import csv
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parent
TARGETS = {'x86': 'i686-pc-windows-msvc', 'x64': 'x86_64-pc-windows-msvc', 'arm64': 'aarch64-pc-windows-msvc'}
KIND = {'FunctionDecl': 'function', 'TypedefDecl': 'typedef', 'RecordDecl': 'record',
        'EnumDecl': 'enum', 'EnumConstantDecl': 'enumerator', 'VarDecl': 'variable'}


def run(clang, args, output):
    with output.open('w', encoding='utf-8') as stream:
        proc = subprocess.run([str(clang), *args], stdout=stream, stderr=subprocess.PIPE, text=True, encoding='utf-8')
    output.with_suffix(output.suffix + '.log').write_text(proc.stderr, encoding='utf-8')
    assert proc.returncode == 0, proc.stderr


def audit(arch, config):
    SDK = Path(config['sdk_include'])
    VC = Path(config['msvc_include'])
    CLANG = Path(config['clang_path'])
    out = ROOT / 'out' / arch
    args = [*config['clang_args'], '--target=' + TARGETS[arch]]
    run(CLANG, [*args, '-Xclang', '-ast-dump=json', '-fsyntax-only', str(ROOT / 'main.h')], out / 'clang-ast.json')
    run(CLANG, [*args, '-E', '-dD', str(ROOT / 'main.h')], out / 'clang-macros.txt')
    run(CLANG, [*args, '-M', str(ROOT / 'main.h')], out / 'clang-dependencies.txt')
    tree = json.loads((out / 'clang-ast.json').read_text(encoding='utf-8'))
    declarations = {}
    last_file = ''

    def walk(node, parent_file='', owner=''):
        nonlocal last_file
        loc = node.get('loc', {})
        if loc.get('spellingLoc', {}).get('file'):
            last_file = loc['spellingLoc']['file'].replace('\\', '/')
        loc = loc.get('expansionLoc', loc)
        if loc.get('file'):
            last_file = loc['file'].replace('\\', '/')
        file = loc.get('file', last_file or parent_file).replace('\\', '/')
        kind, name = node.get('kind'), node.get('name', '')
        selected = '/ucrt/' in file or (file.startswith(str(VC).replace('\\', '/')) and Path(file).name in {'vadefs.h', 'vcruntime.h', 'vcruntime_startup.h'})
        if selected and kind in KIND and name and not node.get('isImplicit'):
            row = {'kind': KIND[kind], 'name': name, 'header': file,
                   'line': loc.get('line'), 'offset': loc.get('offset'),
                   'type': node.get('type', {}).get('qualType'),
                   'variadic': node.get('variadic', False)}
            if kind == 'FunctionDecl':
                row['parameters'] = [{'name': c.get('name', ''), 'type': c.get('type', {}).get('qualType')}
                                     for c in node.get('inner', []) if c.get('kind') == 'ParmVarDecl']
                row['has_body'] = any(c.get('kind') == 'CompoundStmt' for c in node.get('inner', []))
            if kind == 'RecordDecl':
                row['complete'] = node.get('completeDefinition', False)
                row['fields'] = [{'name': c.get('name', ''), 'type': c.get('type', {}).get('qualType')}
                                 for c in node.get('inner', []) if c.get('kind') == 'FieldDecl']
            key = (row['kind'], name)
            if key not in declarations or row.get('complete') or not row.get('has_body', True):
                declarations[key] = row
        for child in node.get('inner', []):
            walk(child, file, name if kind == 'RecordDecl' else owner)

    walk(tree)
    rdl = '\n'.join(p.read_text(encoding='utf-8-sig') for p in (out / 'rdl').glob('*.rdl'))
    names = {
        'function': set(re.findall(r'extern(?:\s+"[^"]+")?\s+fn\s+(?:r#)?(\w+)', rdl)),
        'typedef': set(re.findall(r'\b(?:type|struct|union|enum)\s+(?:r#)?(\w+)', rdl)),
        'record': set(re.findall(r'\b(?:struct|union|type)\s+(?:r#)?(\w+)', rdl)),
        'enum': set(re.findall(r'\b(?:enum|type)\s+(?:r#)?(\w+)', rdl)),
        'enumerator': set(re.findall(r'^\s*(?:r#)?(\w+)\s*=', rdl, re.M)),
        'variable': set(),
    }
    # Names count as represented here, not proof that their signature is correct.
    gaps = []
    for row in declarations.values():
        if row['name'] in names[row['kind']] or (row['kind'] == 'typedef' and row['name'] in names['function']):
            row['status'] = 'represented-name'
            continue
        if row['kind'] == 'variable':
            reason = 'external-variable-not-supported-by-current-adapter'
        elif row['kind'] == 'function' and row.get('has_body'):
            reason = 'inline-body-not-retained'
        elif row['kind'] == 'record' and row['name'] in {'_iobuf', '_heapinfo'} and {'_iobuf': 'FILE', '_heapinfo': '_HEAPINFO'}[row['name']] in names['record']:
            reason = 'tag-renamed-to-public-typedef'
        elif row['kind'] == 'record' and row['name'].startswith('_') and row['name'][1:] in names['record']:
            reason = 'tag-renamed-to-public-typedef'
        else:
            reason = 'unresolved'
        row['status'] = reason
        gaps.append(row)
    macros = []
    current_file = ''
    reached = set()
    for line in (out / 'clang-macros.txt').read_text().splitlines():
        marker = re.match(r'# \d+ "(.*?)"', line)
        if marker:
            current_file = marker[1].replace('\\\\', '/').replace('\\', '/')
            reached.add(current_file.lower())
            continue
        if '/ucrt/' not in current_file.lower():
            continue
        match = re.match(r'#define (\w+)(\([^ ]*\))?(?: (.*))?$', line)
        if match:
            name, params, value = match.groups()
            macros.append({'name': name, 'header': current_file, 'function_like': params is not None, 'value': value or '',
                           'status': 'represented-name-value-unverified' if name in names['enumerator'] else 'not-emitted-or-preprocessor-only'})
    with (out / 'winmd-inventory.tsv').open(encoding='utf-8', newline='') as stream:
        emitted = list(csv.DictReader(stream, delimiter='\t'))
    methods = {r['name']: r for r in emitted if r['kind'] == 'method' and r['owner'] == 'Apis'}
    types = {r['name']: r for r in emitted if r['kind'] == 'type'}
    shape_checks = []
    aliases = {'_iobuf': 'FILE', '_heapinfo': '_HEAPINFO', '_div_t': 'div_t', '_ldiv_t': 'ldiv_t', '_lldiv_t': 'lldiv_t'}
    for row in declarations.values():
        if row['kind'] == 'function':
            actual = methods.get(row['name'])
            status = 'missing-function' if actual is None else 'match'
            if actual and (int(actual['parameter_count']) != len(row['parameters']) or (actual['variadic'] == 'true') != row['variadic']):
                status = 'parameter-count-or-varargs-mismatch'
            shape_checks.append({'kind': 'function-shape', 'name': row['name'], 'status': status})
        elif row['kind'] == 'record' and row.get('complete'):
            actual = types.get(aliases.get(row['name'], row['name']))
            expected = [f['name'] for f in row['fields']]
            status = 'missing-record' if actual is None else 'match'
            if actual and actual['fields'].split(',') != expected:
                status = 'field-name-or-order-mismatch'
            shape_checks.append({'kind': 'record-fields', 'name': row['name'], 'status': status})
    headers = []
    for path in sorted((SDK / 'ucrt').rglob('*.h')):
        headers.append({'header': str(path), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest(),
                        'reached_by_preprocessor': path.as_posix().lower() in reached,
                        'has_ast_declarations': any(r['header'].lower() == path.as_posix().lower() for r in declarations.values()),
                        'policy': 'cpp-only-out-of-scope' if path.name in {'new.h', 'safeint.h', 'safeint_internal.h'} else 'audit-required-if-not-reached'})
    report = {'architecture': arch, 'target': TARGETS[arch],
              'counts': dict(Counter(r['kind'] for r in declarations.values())),
              'status_counts': dict(Counter(r['status'] for r in declarations.values())),
              'declarations': sorted(declarations.values(), key=lambda r: (r['kind'], r['name'])),
              'gaps': sorted(gaps, key=lambda r: (r['kind'], r['name'])),
              'headers': headers, 'macros': macros,
              'winmd_shape_counts': dict(Counter(r['status'] for r in shape_checks)),
              'winmd_shape_checks': shape_checks,
              'limitations': ['macro inventory contains source-filtered definitions, not evaluated values or final macro state',
                              'AST source attribution follows elided location state; full location validation is pending',
                              'WinMD checks compare parameter count, varargs and field names/order; full C-to-metadata type equivalence remains unverified',
                              'unresolved gaps are intentionally retained, not silently allowlisted']}
    (out / 'coverage.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(arch, report['counts'], report['status_counts'])


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('architectures', nargs='*', default=list(TARGETS))
    parser.add_argument('--config', type=Path, default=ROOT / 'out/toolchain.json')
    options = parser.parse_args()
    if not options.config.is_file():
        parser.error('Toolchain configuration missing; run build.ps1 first.')
    config = json.loads(options.config.read_text(encoding='utf-8'))
    for arch in options.architectures:
        if arch not in TARGETS:
            parser.error('Expected x86, x64 or arm64.')
        audit(arch, config)
