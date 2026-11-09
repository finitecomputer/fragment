#!/usr/bin/env python3
"""FIN-463 spike driver: a throwaway e2e person on the spike's branch
deployment, their computer, and the spike's levers. Never prints a secret.

  drive.py signin                 sign the e2e person in (session kept in state.json, mode 600)
  drive.py computer               make their computer (id kept in state)
  drive.py view|wake|sleep|restart
  drive.py op <op> [json]         POST /api/test/spike/computer {computer, op, ...json}
  drive.py sh '<shell>' [ms]      exec `sh -c <shell>` in the container
  drive.py py <file.py> [args..]  exec a local Python file in the container (base64 in argv)
  drive.py r2 <count|empty> <prefix>
"""
import base64, json, os, sys, time, urllib.request, urllib.error

BASE = os.environ.get("SPIKE_BASE", "https://fin463-spike.finite.build")
HERE = os.path.dirname(os.path.abspath(__file__))
STATE = os.path.join(HERE, "state.json")
SECRET_FILE = os.path.expanduser("~/.config/fragment/secrets/preview-test-secret")


def secret():
    return open(SECRET_FILE).read().strip()


def load():
    try:
        return json.load(open(STATE))
    except FileNotFoundError:
        return {}


def save(st):
    fd = os.open(STATE, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.write(fd, json.dumps(st).encode())
    os.close(fd)


def call(method, path, body=None, test=False, session=None, timeout=600):
    headers = {"content-type": "application/json", "user-agent": "fin463-spike"}
    if test:
        headers["x-fragment-test-secret"] = secret()
    if session:
        headers["cookie"] = f"__Host-fragment_session={session}"
        headers["x-fragment-shell"] = "1"
        headers["sec-fetch-site"] = "same-origin"
        headers["origin"] = BASE
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(BASE + path, data=data, method=method, headers=headers)
    t0 = time.time()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            text = r.read().decode()
            status = r.status
    except urllib.error.HTTPError as e:
        text = e.read().decode()
        status = e.code
    ms = int((time.time() - t0) * 1000)
    try:
        value = json.loads(text)
    except Exception:
        value = text[:2000]
    return status, value, ms


def view_brief(v):
    if not isinstance(v, dict):
        return v
    keys = ["computer", "phase", "generation", "image", "version", "notices", "why", "saves"]
    return {k: v.get(k) for k in keys if k in v}


def op(st, name, extra=None, timeout=600):
    body = {"computer": st["computer"], "op": name}
    body.update(extra or {})
    return call("POST", "/api/test/spike/computer", body, test=True, timeout=timeout)


def main():
    st = load()
    cmd = sys.argv[1]
    if cmd == "signin":
        s, v, ms = call("POST", "/api/test/signin", {"email": "fin463-spike@e2e.test"}, test=True)
        if s == 200:
            st["session"] = v["session"]
            st["identity"] = v["identity"]
            save(st)
            print(s, {"identity": v["identity"], "created": v.get("created")}, ms)
        else:
            print(s, v, ms)
    elif cmd == "computer":
        s, v, ms = call("POST", "/api/computers", {}, session=st["session"])
        if s == 200:
            st["computer"] = v["computer"]
            save(st)
        print(s, view_brief(v), ms)
    elif cmd == "view":
        s, v, ms = call("GET", f"/api/computers/{st['computer']}", session=st["session"])
        print(s, json.dumps(v)[:3000] if "-v" in sys.argv else view_brief(v), ms)
    elif cmd in ("wake", "sleep", "restart"):
        s, v, ms = call("POST", f"/api/computers/{st['computer']}/{cmd}", {}, session=st["session"], timeout=900)
        print(s, view_brief(v), ms)
    elif cmd == "op":
        extra = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
        s, v, ms = op(st, sys.argv[2], extra)
        print(s, json.dumps(v, indent=1)[:20000], ms)
    elif cmd == "sh":
        ms_bound = int(sys.argv[3]) if len(sys.argv) > 3 else 60000
        s, v, ms = op(st, "exec", {"argv": ["sh", "-c", sys.argv[2]], "ms": ms_bound})
        if isinstance(v, dict) and "output" in v:
            print(v["output"], end="" if v["output"].endswith("\n") else "\n")
            print(f"[exit {v.get('exitCode')} in {v.get('ms')} ms; round trip {ms} ms; http {s}]")
        else:
            print(s, v, ms)
    elif cmd == "py":
        src = open(sys.argv[2]).read()
        b64 = base64.b64encode(src.encode()).decode()
        boot = f"import base64,sys;sys.argv=['spike']+sys.argv[1:];exec(compile(base64.b64decode('{b64}'),'spike','exec'))"
        argv = ["python3", "-c", boot] + sys.argv[3:]
        s, v, ms = op(st, "exec", {"argv": argv, "ms": 90000})
        if isinstance(v, dict) and "output" in v:
            print(v["output"], end="" if v["output"].endswith("\n") else "\n")
            print(f"[exit {v.get('exitCode')} in {v.get('ms')} ms; round trip {ms} ms; http {s}]")
        else:
            print(s, v, ms)
    elif cmd == "put":
        # put a local file at a path in the container: put <local> <remote>
        b64 = base64.b64encode(open(sys.argv[2], "rb").read()).decode()
        s, v, ms = op(st, "exec", {"argv": ["sh", "-c", f"echo '{b64}' | base64 -d > {sys.argv[3]} && wc -c {sys.argv[3]}"], "ms": 30000})
        print(s, v.get("output", v) if isinstance(v, dict) else v, ms)
    elif cmd == "bg":
        # run a container file in the background: bg <remote.py> <log> [args..]
        args = " ".join(sys.argv[4:])
        s, v, ms = op(st, "exec", {"argv": ["sh", "-c", f"nohup python3 -u {sys.argv[2]} {args} > {sys.argv[3]} 2>&1 & echo started $!"], "ms": 30000})
        print(s, v.get("output", v) if isinstance(v, dict) else v, ms)
    elif cmd == "r2":
        s, v, ms = call("POST", "/api/test/spike/r2", {"op": sys.argv[2], "prefix": sys.argv[3]}, test=True)
        print(s, v, ms)
    else:
        print(__doc__)


if __name__ == "__main__":
    main()
