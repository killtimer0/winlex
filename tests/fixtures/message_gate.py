"""Query generated message DLLs with both native Debug CLIs (no builds or GUI)."""
import argparse
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
MACHINES = {'x86': 0x14c, 'x64': 0x8664}
# Literal expectations from build_message_fixtures.py's authored resource text,
# not FormatMessage, the product parser, or another invocation of the CLI.
TEXT = {0x409: 'Fixture %1!s! message.', 0x804: '测试消息 %1!s!。'}
TEMPLATES = {
    0x20000002: '"0x%08lx" referenced "0x%08lx"; operation "%s".',
    0x20000003: 'ANSI %08lx and %1!s! remain complete.',
}


def require(condition, detail):
    if not condition:
        raise ValueError(detail)


def check_machine(path, arch):
    with path.open('rb') as stream:
        header = stream.read(64)
        require(len(header) == 64 and header[:2] == b'MZ', f'{path}: not PE')
        stream.seek(struct.unpack_from('<I', header, 60)[0])
        signature = stream.read(6)
    require(len(signature) == 6 and signature[:4] == b'PE\0\0'
            and struct.unpack_from('<H', signature, 4)[0] == MACHINES[arch],
            f'{path}: expected {arch} PE')


class Gate:
    def __init__(self):
        self.cases = []

    def query(self, name, cli, module, *, code=0x20000001, lang='0x409',
              status='found', texts=None, actual=None):
        command = [str(cli), 'error-code', hex(code), '--module', str(module),
                   '--lang', lang, '--json']
        case = {'name': name, 'command': command, 'status': 'failed'}
        self.cases.append(case)
        try:
            process = subprocess.run(command, capture_output=True, timeout=15,
                                     creationflags=subprocess.CREATE_NO_WINDOW)
            case.update(exit_code=process.returncode,
                        stdout=process.stdout.decode('utf-8', errors='replace'),
                        stderr=process.stderr.decode('utf-8', errors='replace'))
            expected_exit = 3 if status == 'failed' else 0
            require(process.returncode == expected_exit,
                    f'expected exit {expected_exit}, got {process.returncode}')
            payload = json.loads(process.stdout.decode('utf-8'))
            require(payload['schema'] == 'winlex-cli-1'
                    and payload['command'] == 'error-code'
                    and payload['ok'] is (status != 'failed'), 'response envelope')
            result = payload['result']
            requested = 0 if lang == 'auto' else int(lang, 0)
            require(result['request'] == {
                'code': code, 'language': requested,
                'scope': {'kind': 'module', 'path': str(module)},
            }, 'request round trip')
            require(result['attempts'] == [] and len(result['candidates']) == 1,
                    'module query must not fall back to system sources')
            candidate = result['candidates'][0]
            require(candidate['scheme'] == 'module' and candidate['related'] == [],
                    'module candidate only')
            message = candidate['message']
            require(message['source'] == {'kind': 'module', 'path': str(module)}
                    and message['code'] == code
                    and message['requested_language'] == requested,
                    'message provenance / full ID / requested language')
            require(message['status'] == status, f'expected {status}: {message}')
            if status == 'found':
                require(message['failure'] is None, 'unexpected lookup failure')
                require(message['actual_language'] in texts
                        and message['text'] == texts[message['actual_language']],
                        f'wrong selected language or raw template: {message}')
            else:
                require(message['text'] is None and message['actual_language'] == actual,
                        'non-found result must not fabricate text/language')
                if status == 'failed':
                    require(isinstance(message['failure'], dict)
                            and isinstance(message['failure']['os_code'], int)
                            and message['failure']['os_code'] != 0,
                            'load failure must report a Windows error')
                else:
                    require(message['failure'] is None, 'ordinary miss is not a failure')
            case['status'] = 'passed'
        except (OSError, ValueError, KeyError, TypeError, subprocess.TimeoutExpired) as error:
            case['error'] = str(error)


def run(args, gate, scratch):
    fixtures = args.fixtures.resolve()
    clis = {'x86': args.x86_cli.resolve(), 'x64': args.x64_cli.resolve()}
    for arch, cli in clis.items():
        check_machine(cli, arch)
    for arch in MACHINES:
        for stem in ('messages', 'neutral', 'empty'):
            check_machine(fixtures / f'{stem}-{arch}.dll', arch)
    if args.unc_fixtures:
        require(str(args.unc_fixtures).startswith('\\\\')
                and not str(args.unc_fixtures).startswith('\\\\?\\'),
                '--unc-fixtures must be an ordinary UNC share path')
        for arch in MACHINES:
            remote = args.unc_fixtures / f'messages-{arch}.dll'
            require(remote.read_bytes() == (fixtures / remote.name).read_bytes(),
                    f'{remote}: UNC fixture differs')

    corrupt = scratch / 'corrupt.dll'
    corrupt.write_bytes(b'MZ' + bytes(126))
    unicode_dir = scratch / '消息资源 空格'
    unicode_dir.mkdir()
    long_dir = unicode_dir
    while len(str(long_dir)) < 280:
        long_dir /= 'long-resource-directory'
    extended = Path('\\\\?\\' + str(long_dir))
    extended.mkdir(parents=True)
    for process_arch, cli in clis.items():
        for resource_arch in MACHINES:
            prefix = f'{process_arch}/{resource_arch}'
            module = fixtures / f'messages-{resource_arch}.dll'
            for language in (0x409, 0x804):
                gate.query(f'{prefix}/exact-{language:04x}', cli, module,
                           lang=hex(language), texts={language: TEXT[language]})
                for code, text in TEMPLATES.items():
                    gate.query(f'{prefix}/template-{code:x}-{language:04x}', cli, module,
                               code=code, lang=hex(language), texts={language: text})
            # Automatic selection depends on the host, but must report the actual
            # language and matching text from one of the two authored resources.
            for language in ('auto', '0'):
                gate.query(f'{prefix}/automatic-{language}', cli, module,
                           lang=language, texts=TEXT)
            gate.query(f'{prefix}/missing-language', cli, module,
                       lang='0x411', status='language_not_found')
            gate.query(f'{prefix}/missing-message', cli, module,
                       code=0x20000004, status='not_found')
            # ID 5 exists in system messages: no module-to-system fallback.
            gate.query(f'{prefix}/no-system-fallback', cli, module,
                       code=5, status='not_found')
            neutral = fixtures / f'neutral-{resource_arch}.dll'
            gate.query(f'{prefix}/neutral-auto', cli, neutral,
                       lang='auto', texts={0: TEXT[0x409]})
            gate.query(f'{prefix}/neutral-zero', cli, neutral,
                       lang='0', texts={0: TEXT[0x409]})
            gate.query(f'{prefix}/neutral-strict', cli, neutral,
                       status='language_not_found')
            gate.query(f'{prefix}/empty', cli, fixtures / f'empty-{resource_arch}.dll',
                       status='no_message_table')
            for label, directory in (('unicode', unicode_dir), ('long', extended)):
                copy = directory / module.name
                shutil.copyfile(module, copy)
                gate.query(f'{prefix}/{label}', cli, copy, texts={0x409: TEXT[0x409]})
            if args.unc_fixtures:
                gate.query(f'{prefix}/unc', cli, args.unc_fixtures / module.name,
                           texts={0x409: TEXT[0x409]})
        gate.query(f'{process_arch}/missing-module', cli, scratch / 'missing.dll',
                   status='failed')
        gate.query(f'{process_arch}/corrupt-module', cli, corrupt, status='failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--x86-cli', required=True, type=Path)
    parser.add_argument('--x64-cli', required=True, type=Path)
    parser.add_argument('--fixtures', required=True, type=Path)
    parser.add_argument('--unc-fixtures', type=Path,
                        help='existing UNC directory containing the same message DLLs')
    parser.add_argument('--report', type=Path,
                        default=ROOT / 'tmp/verify/messages/report.json')
    args = parser.parse_args()
    gate = Gate()
    report = {'cases': gate.cases, 'skipped': []}
    try:
        require(os.name == 'nt', 'requires Windows with x86 and x64 process support')
        if not args.unc_fixtures:
            report['skipped'].append('UNC: provide --unc-fixtures with an existing share')
        (ROOT / 'tmp').mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix='message-gate-', dir=ROOT / 'tmp') as temp:
            run(args, gate, Path(temp).resolve())
    except (OSError, ValueError) as error:
        report['setup_error'] = str(error)
    report['passed'] = (bool(gate.cases) and 'setup_error' not in report
                        and all(case['status'] == 'passed' for case in gate.cases))
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n',
                           encoding='utf-8')
    passed = sum(case['status'] == 'passed' for case in gate.cases)
    print(f'{passed}/{len(gate.cases)} passed; {len(report["skipped"])} skipped; {args.report}')
    if 'setup_error' in report:
        print(report['setup_error'])
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
