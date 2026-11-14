"""Read append-only journals, including tails left by a killed writer."""
import json
from pathlib import Path


def records(path):
    """Every committed record; a killed writer can leave a split UTF-8 tail."""
    path = Path(path)
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_bytes().splitlines(keepends=True)
            if line.endswith(b'\n') and line.strip()]


def last(path, tail=65536):
    """Last parseable object in the final `tail` bytes, or None."""
    try:
        with Path(path).open('rb') as stream:
            stream.seek(0, 2)
            stream.seek(max(0, stream.tell() - tail))
            lines = stream.read().splitlines()
    except OSError:
        return None
    for line in reversed(lines):
        try:
            record = json.loads(line)
        except ValueError:
            continue
        if isinstance(record, dict):
            return record
    return None


def committed(path, tail=65536):
    """Last fully written record, ignoring a partially written tail."""
    try:
        with Path(path).open('rb') as stream:
            stream.seek(0, 2)
            stream.seek(max(0, stream.tell() - tail))
            lines = stream.read().splitlines(keepends=True)
    except FileNotFoundError:
        return None
    complete = [line for line in lines if line.endswith(b'\n')]
    return json.loads(complete[-1]) if complete else None
