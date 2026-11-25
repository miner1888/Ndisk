import pathlib, subprocess, tempfile, shutil, sys

src = pathlib.Path(__file__).resolve().parent.parent / "web" / "js"
tmp = pathlib.Path(tempfile.mkdtemp(prefix="ndv2_js_"))
fails = 0
for f in sorted(src.rglob("*.js")):
    t = tmp / f.name
    t.write_bytes(f.read_bytes())
    r = subprocess.run(["node", "--check", str(t)], capture_output=True, text=True)
    status = "OK" if r.returncode == 0 else "FAIL: " + r.stderr.strip()[:300]
    if r.returncode != 0:
        fails += 1
    print(f.as_posix(), status)
shutil.rmtree(tmp, ignore_errors=True)
print("fails:", fails)
sys.exit(1 if fails else 0)
