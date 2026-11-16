"""A bounded streaming source owned by the PTY load/quit test."""

import json
import os
from pathlib import Path
import sys
import time


marker = Path(sys.argv[1])
pending = marker.with_suffix(".pending")
pending.write_text(json.dumps({"pid": os.getpid(), "group": os.getpgrp()}))
pending.replace(marker)
deadline = time.monotonic() + 30
try:
    while time.monotonic() < deadline:
        sys.stdout.write("".join(f"row {row:06d}\n" for row in range(1000)))
        sys.stdout.flush()
        time.sleep(0.01)
except BrokenPipeError:
    os._exit(0)
