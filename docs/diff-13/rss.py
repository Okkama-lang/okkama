#!/usr/bin/env python3
"""Peak process RSS for one pure bounded benchmark; macOS reports bytes."""
import json
import os
import subprocess
import sys
import time
start = time.monotonic()
process = subprocess.Popen(sys.argv[1:])
_, status, usage = os.wait4(process.pid, 0)
process.returncode = os.waitstatus_to_exitcode(status)
print(json.dumps(dict(exit_code=process.returncode, wall_seconds=time.monotonic() - start,
    max_rss_bytes=usage.ru_maxrss, user_seconds=usage.ru_utime, system_seconds=usage.ru_stime)), file=sys.stderr)
sys.exit(process.returncode)
