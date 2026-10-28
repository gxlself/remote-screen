#!/usr/bin/env python3
"""Copy generated files only when bytes differ, preserving unchanged mtimes."""
from pathlib import Path
import shutil
import sys

source, destination = (Path(value) for value in sys.argv[1:])
for path in sorted(source.rglob('*')):
    if not path.is_file():
        continue
    output = destination / path.relative_to(source)
    if output.is_file() and output.read_bytes() == path.read_bytes():
        continue
    output.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(path, output)
