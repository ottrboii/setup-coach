import glob, os, re, subprocess, sys
vs = subprocess.run([os.path.expandvars(r"%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"), "-latest", "-find", r"**\Hostx64\x64\dumpbin.exe"], capture_output=True, text=True).stdout.split("\n")[0].strip()
print("dumpbin:", vs)
exes = sorted(glob.glob(r"target\release\deps\setup_coach_app-*.exe"), key=os.path.getmtime)
exe = exes[-1]
print("exe:", exe)
out = subprocess.run([vs, "/imports", exe], capture_output=True, text=True).stdout
cur = None; imps = {}
for line in out.splitlines():
    m = re.match(r"^\s{4}(\S+\.dll)\s*$", line, re.I)
    if m: cur = m.group(1).lower(); imps[cur] = []; continue
    m = re.match(r"^\s+[0-9A-F]+\s+(\S+)\s*$", line) or re.match(r"^\s+[0-9A-F]+\s+[0-9A-F]+\s+(\S+)\s*$", line)
    if cur and m: imps[cur].append(m.group(1))
for dll, names in imps.items():
    if dll.startswith(("api-ms", "vcruntime", "ucrt")): continue
    path = os.path.join(os.environ["WINDIR"], "System32", dll)
    if not os.path.exists(path): print("no file", dll); continue
    ex = subprocess.run([vs, "/exports", path], capture_output=True, text=True).stdout
    missing = [n for n in names if n not in ex]
    print(dll, len(names), "missing:", missing[:20])
