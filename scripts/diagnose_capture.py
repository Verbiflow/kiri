import collections
import json
import os
import subprocess
import sys
import time

root = sys.argv[1]
started = time.monotonic()
env = dict(os.environ, GIT_OPTIONAL_LOCKS="0", LC_ALL="C")
base = ["git", "--no-optional-locks", "--literal-pathspecs", "-C", root]
def git(args, data=None):
    return subprocess.run(base + args, input=data, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env, check=True, timeout=30).stdout

raw = git(["diff", "--cached", "--raw", "--no-abbrev", "--no-renames", "-z"])
fields = raw.split(b"\0")
files = []
for i in range(0, len(fields) - 1, 2):
    if not fields[i]:
        continue
    header = fields[i].split()
    files.append({"path": fields[i + 1], "old": header[2], "new": header[3], "status": header[4].decode("ascii")})
ids = sorted({file[side] for file in files for side in ("old", "new") if set(file[side]) != {48}})
sizes = {}
if ids:
    for line in git(["cat-file", "--batch-check=%(objectname) %(objecttype) %(objectsize)"], b"\n".join(ids) + b"\n").splitlines():
        oid, kind, size = line.split()
        sizes[oid] = int(size)
extensions = collections.defaultdict(lambda: {"files": 0, "new_bytes": 0, "old_bytes": 0})
for file in files:
    file["new_bytes"] = sizes.get(file["new"], 0)
    file["old_bytes"] = sizes.get(file["old"], 0)
    extension = os.path.splitext(os.fsdecode(file["path"]))[1] or "[no extension]"
    group = extensions[extension]
    group["files"] += 1
    group["new_bytes"] += file["new_bytes"]
    group["old_bytes"] += file["old_bytes"]
largest = sorted(files, key=lambda file: file["new_bytes"] + file["old_bytes"], reverse=True)[:20]
print(json.dumps({"metadata_seconds": time.monotonic() - started, "staged_files": len(files), "status_counts": dict(collections.Counter(file["status"] for file in files)), "total_new_blob_bytes": sum(file["new_bytes"] for file in files), "total_old_blob_bytes": sum(file["old_bytes"] for file in files), "largest": [{"path": os.fsdecode(file["path"]), "status": file["status"], "new_bytes": file["new_bytes"], "old_bytes": file["old_bytes"]} for file in largest], "extensions": dict(sorted(extensions.items(), key=lambda pair: pair[1]["new_bytes"], reverse=True)), "scope": "Blob metadata only. No patches, model calls, staging or commits. Blob totals are not patch sizes."}, indent=2))
