"""SDK layouts and the public per-document architecture selection contract."""
import re

from driver import until


# Windows SDK SECURITY_ATTRIBUTES: DWORD, void *, BOOL. These ABI expectations
# are independent of Winlex's formatter and the architecture of the GUI process.
LAYOUTS = {
    'x86': (12, 4, ((0, 4), (4, 4), (8, 4))),
    'x64': (24, 8, ((0, 4), (8, 8), (16, 4))),
    'arm64': (24, 8, ((0, 4), (8, 8), (16, 4))),
}


def wait_layout(document, architecture):
    size, alignment, fields = LAYOUTS[architecture]

    def matches():
        text = document.text_block().replace('\r\n', '\n')
        assert 'struct SECURITY_ATTRIBUTES' in text
        assert 'size: {} bytes, alignment: {} bytes'.format(size, alignment) in text
        for name, (offset, width) in zip(
                ('nLength', 'lpSecurityDescriptor', 'bInheritHandle'), fields):
            assert re.search(
                r'// offset: {} bytes, size: {} bytes\n\s*[^;\n]*\b{}\s*;'.format(
                    offset, width, name), text), text
        assert document.architecture() == architecture
        return True

    until('SECURITY_ATTRIBUTES {} layout'.format(architecture), matches)


def test_switch_architecture_updates_sdk_layout(app):
    document = app.open_symbol('SECURITY_ATTRIBUTES')
    for architecture in ('x86', 'x64', 'arm64', 'x86'):
        document.select_architecture(architecture)
        wait_layout(document, architecture)


def test_documents_keep_independent_architectures(app):
    original = app.open_symbol('SECURITY_ATTRIBUTES')
    original.select_architecture('x86')
    wait_layout(original, 'x86')

    duplicate = app.open_symbol('SECURITY_ATTRIBUTES', new=True)
    assert duplicate.window() != original.window()
    duplicate.select_architecture('x64')
    wait_layout(duplicate, 'x64')
    original.activate()
    wait_layout(original, 'x86')

    original.select_architecture('arm64')
    wait_layout(original, 'arm64')
    duplicate.activate()
    wait_layout(duplicate, 'x64')
    original.activate()
    wait_layout(original, 'arm64')
