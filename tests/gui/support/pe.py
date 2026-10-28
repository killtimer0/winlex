"""PE machine detection shared by the runner and process sessions."""
from pathlib import Path
import struct

PE_HEADER_OFFSET = 0x3C
IMAGE_FILE_MACHINE_AMD64 = 0x8664


def machine(path):
    data = Path(path).read_bytes()
    header = struct.unpack_from("<I", data, PE_HEADER_OFFSET)[0]
    return struct.unpack_from("<H", data, header + 4)[0]


def is_64_bit(path):
    return machine(path) == IMAGE_FILE_MACHINE_AMD64
