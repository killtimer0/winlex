"""Read MSVC PE/CodeView and MSF 7 PDB identities without external tools."""
import struct


def pe_identity(data):
    pe = struct.unpack_from("<I", data, 0x3c)[0]
    optional = pe + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    directories = optional + (112 if magic == 0x20b else 96)
    rva, size = struct.unpack_from("<II", data, directories + 6 * 8)
    sections = struct.unpack_from("<H", data, pe + 6)[0]
    table = optional + struct.unpack_from("<H", data, pe + 20)[0]
    for index in range(sections):
        virtual, address, raw_size, raw = struct.unpack_from("<IIII", data, table + index * 40 + 8)
        if address <= rva < address + max(virtual, raw_size):
            start = raw + rva - address
            for entry in range(start, start + size, 28):
                kind, length, _, offset = struct.unpack_from("<IIII", data, entry + 12)
                if kind == 2 and length >= 24 and data[offset:offset + 4] == b"RSDS":
                    return data[offset + 4:offset + 20], struct.unpack_from("<I", data, offset + 20)[0]
    raise ValueError("Missing RSDS executable debug identity")


def pdb_identity(data):
    if data[:32] != b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0":
        raise ValueError("Unsupported PDB format")
    block, _, count, directory_size, _, block_map = struct.unpack_from("<6I", data, 32)
    if block < 512 or block & (block - 1) or count * block != len(data):
        raise ValueError("Invalid PDB block layout")

    def blocks(indices):
        if any(index >= count for index in indices):
            raise ValueError("Invalid PDB block index")
        return b"".join(data[index * block:(index + 1) * block] for index in indices)

    directory_blocks = (directory_size + block - 1) // block
    directory = blocks(struct.unpack_from(f"<{directory_blocks}I", data, block_map * block))[:directory_size]
    streams = struct.unpack_from("<I", directory)[0]
    if streams < 2:
        raise ValueError("Missing PDB info stream")
    sizes = struct.unpack_from(f"<{streams}I", directory, 4)
    cursor = 4 + streams * 4
    for index, size in enumerate(sizes[:2]):
        length = 0 if size == 0xffffffff else (size + block - 1) // block
        indices = struct.unpack_from(f"<{length}I", directory, cursor)
        cursor += length * 4
        if index == 1:
            info = blocks(indices)[:size]
            if len(info) < 28:
                raise ValueError("Truncated PDB info stream")
            return info[12:28], struct.unpack_from("<I", info, 8)[0]


def verify(executable, pdb):
    if pe_identity(executable) != pdb_identity(pdb):
        raise ValueError("Executable/PDB identity mismatch")
