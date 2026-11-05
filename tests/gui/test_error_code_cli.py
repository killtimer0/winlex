"""Black-box tool contracts, runnable on the remote Windows test workers."""
import json
import os
import subprocess
import shutil

import pytest


def query(settings, *arguments, expected=0):
    cli = settings[0].with_name('winlex.exe')
    result = subprocess.run([str(cli), 'error-code', *map(str, arguments), '--json'],
                            capture_output=True, timeout=30)
    if result.returncode != expected:
        (settings[2] / 'error-code-cli-failure.stdout').write_bytes(result.stdout)
        (settings[2] / 'error-code-cli-failure.stderr').write_bytes(result.stderr)
    assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
    payload = json.loads(result.stdout.decode('utf-8'))
    assert payload['ok'] == (expected == 0), payload
    return payload


def test_error_code_cli_queries_without_metadata(settings):
    result = query(settings, '-2147024891', '--scope', 'hresult', '--lang', '0x409')['result']
    assert result['request']['code'] == 0x80070005
    assert not {'hex', 'unsigned', 'signed'} & result.keys()
    candidate = result['candidates'][0]
    assert candidate['scheme'] == 'h_result'
    assert candidate['message']['code'] == 0x80070005
    related = candidate['related'][0]
    assert 'code' not in related
    assert related['message']['code'] == 5
    assert related['message']['status'] == 'found'
    assert related['message']['actual_language'] == 0x409
    assert related['message']['text']
    auto = query(settings, '5')['result']
    assert auto['candidates']
    assert all(c['message']['actual_language'] is None for c in auto['candidates']
               if c['message']['source']['kind'] == 'system')
    assert query(settings, '010', '--scope', 'win32')['result']['request']['code'] == 8


def test_error_code_cli_text_and_help_without_metadata(settings):
    cli = str(settings[0].with_name('winlex.exe'))
    result = subprocess.run([cli, 'error-code', '0x80070005', '--scope', 'hresult',
                             '--lang', '0x409'], capture_output=True, timeout=30)
    assert result.returncode == 0, result.stderr
    text = result.stdout.decode('utf-8')
    assert '0x80070005' in text and '-2147024891' in text
    assert '0x00000005' in text and 'Related Win32' in text
    assert 'Access is denied' in text
    help_result = subprocess.run([cli, 'error-code', '--help'],
                                 capture_output=True, timeout=30)
    assert help_result.returncode == 0
    assert b'--module' in help_result.stdout and b'--lang' in help_result.stdout


def test_error_code_cli_errno_is_explicit_english_and_runtime_scoped(settings):
    for language in ('auto', '0x409'):
        result = query(settings, 2, '--scope', 'errno', '--lang', language)['result']
        assert result['request']['scope']['kind'] == 'errno'
        assert not result['attempts'] and len(result['candidates']) == 1
        candidate = result['candidates'][0]
        assert candidate['scheme'] == 'errno'
        assert candidate['encoding'] is None and not candidate['related']
        message = candidate['message']
        assert message['source']['kind'] == 'msvcrt'
        assert message['status'] == 'found' and message['text'] == 'No such file or directory'
        assert message['actual_language'] == 0x409
    assert query(settings, 0, '--scope', 'errno')['result']['candidates'][0]['message']['text'] == 'No error'
    for value in (15, 100, -1, 2147483648):
        message = query(settings, value, '--scope', 'errno')['result']['candidates'][0]['message']
        assert message['status'] == 'not_found' and message['text'] is None
        assert message['failure'] is None
    message = query(settings, 2, '--scope', 'errno', '--lang', '0x804')['result']['candidates'][0]['message']
    assert message['status'] == 'language_not_found' and message['text'] is None
    auto = query(settings, 2)['result']
    assert all(c['scheme'] != 'errno' for c in auto['candidates'])
    assert all(m['source']['kind'] != 'msvcrt' for m in auto['attempts'])
    query(settings, 2, '--scope', 'errno', '--module', 'ignored.dll', expected=2)
    text = subprocess.run([str(settings[0].with_name('winlex.exe')), 'error-code', '2',
                           '--scope', 'errno'], capture_output=True, timeout=30)
    assert text.returncode == 0
    assert b'msvcrt.dll' in text.stdout and b'No such file or directory' in text.stdout


def test_error_code_cli_missing_and_invalid_are_distinct(settings):
    missing = query(settings, '0x12345678')['result']
    assert not missing['candidates']
    assert missing['attempts']
    for arguments in [('4294967296',), ('-2147483649',), ('5.0',),
                      ('ERROR_ACCESS_DENIED',), ('5', '--lang', '-1'),
                      ('5', '--lang', '65536')]:
        result = query(settings, *arguments, expected=2)
        assert result['error']['code'] == 'invalid_arguments'


def test_error_code_cli_missing_module_failure(settings):
    failed = query(settings, '5', '--module', settings[2] / 'missing.dll', expected=3)
    message = failed['result']['candidates'][0]['message']
    assert message['status'] == 'failed'
    assert message['failure']['os_code'] is not None


def test_error_code_cli_ntstatus_preserves_complete_template(settings):
    module = os.path.join(os.environ['SystemRoot'], 'System32', 'ntdll.dll')
    messages = []
    for scope in [('--scope', 'ntstatus'), ('--module', module)]:
        result = query(settings, '0xC0000005', *scope, '--lang', 'auto')['result']
        message = result['candidates'][0]['message']
        messages.append(message)
        (settings[2] / 'ntstatus-complete-templates.json').write_text(
            json.dumps(messages, ensure_ascii=False, indent=2), encoding='utf-8')
        assert message['status'] == 'found'
        if scope[0] == '--module':
            assert message['source']['kind'] == 'module'
        text = message['text']
        assert 'instruction' in text.lower() or '指令' in text, text
        assert 'referenced memory' in text.lower() or '内存' in text, text
        assert 'could not be' in text.lower() or '不能' in text, text
        assert text.count('%') >= 3 and '%s' in text, text
        assert text.endswith(('.', '。')), text


def test_error_code_cli_module_preserves_exact_templates(settings):
    for arch in ('x86', 'x64'):
        module = settings[0].with_name('messages-' + arch + '.dll')
        if not module.exists():
            pytest.skip('resource-only x86/x64 message fixtures were not deployed')
        for code, expected in [
            ('0x20000001', 'Fixture %1!s! message.'),
            ('0x20000002', '"0x%08lx" referenced "0x%08lx"; operation "%s".'),
            ('0x20000003', 'ANSI %08lx and %1!s! remain complete.'),
        ]:
            message = query(settings, code, '--module', module,
                            '--lang', '0x409')['result']['candidates'][0]['message']
            assert message['status'] == 'found'
            assert message['text'] == expected
            assert message['actual_language'] == 0x409


def test_error_code_cli_module_without_message_table(settings):
    result = query(settings, '5', '--module', settings[0])['result']
    message = result['candidates'][0]['message']
    assert message['status'] == 'no_message_table'
    assert message['text'] is None


def test_error_code_cli_neutral_resource_language_is_reported(settings):
    module = settings[0].with_name('neutral-x86.dll')
    if not module.exists():
        pytest.skip('neutral message resource fixture was not deployed')
    message = query(settings, '0x20000002', '--module', module)['result']['candidates'][0]['message']
    assert message['actual_language'] == 0
    assert message['text'] == '"0x%08lx" referenced "0x%08lx"; operation "%s".'
    strict = query(settings, '0x20000002', '--module', module,
                   '--lang', '0x409')['result']['candidates'][0]['message']
    assert strict['status'] == 'language_not_found'
    text = subprocess.run([str(settings[0].with_name('winlex.exe')), 'error-code',
                           '0x20000002', '--module', str(module)],
                          capture_output=True, timeout=30)
    assert text.returncode == 0
    assert b'Actual language: neutral resource (0x0000)' in text.stdout


def test_error_code_cli_custom_module_contract(settings):
    """Additional resource fixtures are deployed by the validation runner."""
    paths = [settings[0].with_name('messages-' + arch + '.dll') for arch in ('x86', 'x64')]
    if not all(path.exists() for path in paths):
        pytest.skip('resource-only x86/x64 message fixtures were not deployed')
    for module in paths:
        for language, word in [('0x409', 'Fixture'), ('0x804', '测试消息')]:
            result = query(settings, '0x20000001', '--module', module,
                           '--lang', language)['result']
            message = result['candidates'][0]['message']
            assert message['status'] == 'found'
            assert word in message['text'] and '%1' in message['text']
            assert message['actual_language'] == int(language, 16)
        missing = query(settings, '5', '--module', module)['result']
        assert missing['candidates'][0]['message']['status'] == 'not_found'
        lang = query(settings, '0x20000001', '--module', module,
                     '--lang', '0x411')['result']
        assert lang['candidates'][0]['message']['status'] == 'language_not_found'
        replacement = module.with_suffix('.renamed')
        module.rename(replacement)
        replacement.rename(module)


def test_error_code_cli_long_module_path(settings):
    source = settings[0].with_name('messages-x64.dll')
    if not source.exists():
        pytest.skip('resource fixture was not deployed')
    folder = str(settings[2] / ('message-path-' + 'a' * 90) / ('b' * 90) / ('c' * 90))
    extended = '\\\\?\\' + folder
    os.makedirs(extended)
    target = extended + '\\messages.dll'
    try:
        shutil.copyfile(str(source), target)
        message = query(settings, '0x20000001', '--module', folder + '\\messages.dll',
                        '--lang', '0x409')['result']['candidates'][0]['message']
        assert message['status'] == 'found'
    finally:
        shutil.rmtree('\\\\?\\' + str(settings[2] / ('message-path-' + 'a' * 90)))


def test_error_code_cli_unc_module_path(settings):
    source = str(settings[0].with_name('messages-x86.dll'))
    drive, tail = os.path.splitdrive(source)
    if not drive or not os.path.isfile(source):
        pytest.skip('resource fixture on a drive was not deployed')
    unc = '\\\\localhost\\' + drive[0] + '$' + tail
    if not os.path.isfile(unc):
        pytest.skip('worker cannot access its local administrative share')
    message = query(settings, '0x20000001', '--module', unc,
                    '--lang', '0x409')['result']['candidates'][0]['message']
    assert message['status'] == 'found'
