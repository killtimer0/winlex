"""Publish whole files so concurrent readers never observe a partial write."""
import json
from pathlib import Path
import time


def write_text(path, text):
    """Replace `path` with `text`, retrying while another owner holds it open."""
    path = Path(path)
    temporary = path.with_name(path.name + '.new')
    temporary.write_text(text, encoding='utf-8')
    for attempt in range(10):
        try:
            temporary.replace(path)
            return
        except PermissionError:
            if attempt == 9:
                raise
            time.sleep(.02)


def write_json(path, value, indent=2):
    write_text(path, json.dumps(value, indent=indent))
