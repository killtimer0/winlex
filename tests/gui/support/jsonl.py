"""Append and read newline-committed JSON journals.

A newline is the commit boundary, not merely a separator: readers must never
accept a complete-looking JSON object whose writer has not finished its line.
An interrupted UTF-8 tail is therefore ignored without decoding it.
"""
import json
from pathlib import Path


def append(path, value):
    """Append one committed value without replacing a Windows reader's file."""
    with Path(path).open('a', encoding='utf-8', newline='\n') as stream:
        stream.write(json.dumps(value) + '\n')


def records(path):
    """Every committed record; a killed writer can leave a split UTF-8 tail."""
    path = Path(path)
    try:
        contents = path.read_bytes()
    except FileNotFoundError:
        return []
    return [json.loads(line) for line in contents.splitlines(keepends=True)
            if line.endswith(b'\n') and line.strip()]


def last(path, tail=65536):
    """Last committed object (including records longer than ``tail``)."""
    value = committed(path, tail)
    return value if isinstance(value, dict) else None


def committed(path, tail=65536):
    """Last fully written value, ignoring a partially written UTF-8 tail."""
    try:
        with Path(path).open('rb') as stream:
            stream.seek(0, 2)
            end = stream.tell()
            size = max(1, tail)
            while True:
                start = max(0, end - size)
                stream.seek(start)
                lines = stream.read(end - start).splitlines(keepends=True)
                if start:
                    # The first line may start inside JSON or a UTF-8 sequence.
                    lines = lines[1:]
                complete = [line for line in lines if line.endswith(b'\n') and line.strip()]
                if complete:
                    return json.loads(complete[-1])
                if not start:
                    return None
                size *= 2
    except FileNotFoundError:
        return None
