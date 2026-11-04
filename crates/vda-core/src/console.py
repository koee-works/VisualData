# A console beside a Python step: the step's inputs stay loaded between runs.
#
# Requests on stdin: a line "RUN <id> <bytes>" or "SAVE <id> <bytes>" followed
# by that many bytes (code, or a path). Each reply is one line on the original
# stdout, starting with \x1eVDA. Everything the code prints is captured into
# the reply, so it cannot be mistaken for a reply.
import ast, io, json, os, sys, traceback, contextlib, datetime, math

proto = os.fdopen(os.dup(1), "w", encoding="utf-8")
# The code's own prints and any stray output from libraries go to a file,
# not the reply channel.
os.dup2(os.open("stray.txt", os.O_WRONLY | os.O_CREAT | os.O_TRUNC), 1)
sys.stdout = io.TextIOWrapper(os.fdopen(1, "wb", 0), encoding="utf-8", write_through=True)


def reply(obj):
    proto.write("\x1eVDA " + json.dumps(obj, default=str, allow_nan=False) + "\n")
    proto.flush()


def cell(v):
    if v is None:
        return None
    try:
        import pandas as pd
        if pd.isna(v):
            return None
    except (TypeError, ValueError, ImportError):
        pass
    if hasattr(v, "item") and not isinstance(v, (str, bytes)):
        try:
            v = v.item()
        except (ValueError, AttributeError):
            pass
    if isinstance(v, bool) or isinstance(v, int) or isinstance(v, str):
        return v
    if isinstance(v, float):
        return v if math.isfinite(v) else str(v)
    if isinstance(v, (datetime.date, datetime.datetime)) or hasattr(v, "isoformat"):
        return v.isoformat()
    return str(v)


def table(df, limit=200):
    head = df.head(limit)
    return {
        "columns": [[str(c), str(t)] for c, t in zip(df.columns, df.dtypes)],
        "rows": [[cell(v) for v in row] for row in head.itertuples(index=False, name=None)],
        "total": int(len(df)),
    }


def summary(v):
    import pandas as pd
    if isinstance(v, pd.DataFrame):
        return "data frame", f"{len(v):,} rows × {len(v.columns)} columns"
    if isinstance(v, pd.Series):
        return "column", f"{len(v):,} values, {v.dtype}"
    if isinstance(v, (bool, int, float, str)):
        r = repr(v)
        return type(v).__name__, r if len(r) <= 60 else r[:57] + "…"
    if isinstance(v, (list, tuple, dict, set)):
        return type(v).__name__, f"{len(v)} items"
    return type(v).__name__, ""


def environment(ns):
    out = []
    for name, v in sorted(ns.items()):
        if name.startswith("_") or name in ("pd",) or type(v).__name__ in ("module", "function", "builtin_function_or_method", "type"):
            continue
        kind, text = summary(v)
        out.append({"name": name, "kind": kind, "summary": text})
    return out


def figures(seq):
    if "matplotlib.pyplot" not in sys.modules:
        return []
    plt = sys.modules["matplotlib.pyplot"]
    paths = []
    for n, num in enumerate(plt.get_fignums()):
        fig = plt.figure(num)
        path = os.path.abspath(f"plot-{seq}-{n}.png")
        fig.savefig(path, dpi=110, bbox_inches="tight")
        paths.append(path)
    plt.close("all")
    return paths


def run(rid, code, ns):
    out, err = io.StringIO(), io.StringIO()
    value, error = None, None
    try:
        tree = ast.parse(code, "<console>", "exec")
        last = tree.body.pop() if tree.body and isinstance(tree.body[-1], ast.Expr) else None
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            exec(compile(tree, "<console>", "exec"), ns)
            v = eval(compile(ast.Expression(last.value), "<console>", "eval"), ns) if last else None
        import pandas as pd
        if isinstance(v, pd.DataFrame):
            value = {"table": table(v)}
        elif isinstance(v, pd.Series):
            value = {"table": table(v.to_frame())}
        elif v is not None:
            r = repr(v)
            value = {"text": r if len(r) <= 20000 else r[:20000] + "…"}
    except SyntaxError as e:
        error = {"message": f"syntax error: {e.msg}", "line": e.lineno}
    except KeyboardInterrupt:
        error = {"message": "interrupted", "line": None}
    except BaseException as e:
        frames = [f for f in traceback.extract_tb(e.__traceback__) if f.filename == "<console>"]
        error = {"message": f"{type(e).__name__}: {e}", "line": frames[-1].lineno if frames else None}
    reply({
        "id": rid, "ok": error is None, "stdout": out.getvalue()[-65536:], "stderr": err.getvalue()[-65536:],
        "value": value, "error": error, "images": figures(rid), "env": environment(ns),
    })


def main():
    pairs = sys.argv[1:]
    try:
        import pandas as pd
        dfs = {pairs[i]: pd.read_parquet(pairs[i + 1], dtype_backend="numpy_nullable") for i in range(0, len(pairs), 2)}
        def param(kind, v):
            return int(v) if kind == "int" else datetime.date.fromisoformat(v) if kind == "date" else v
        with open("params.json", encoding="utf-8") as f:
            params = {name: param(kind, v) for name, kind, v in json.load(f)}
        ns = {"df": dfs[pairs[0]], "dfs": dfs, "pd": pd, "params": params, "__name__": "__console__"}
    except BaseException as e:
        reply({"id": 0, "ok": False, "error": {"message": f"could not load the inputs: {type(e).__name__}: {e}", "line": None}})
        return
    reply({"id": 0, "ok": True, "version": sys.version.split()[0], "env": environment(ns)})
    stdin = sys.stdin.buffer
    while True:
        try:
            header = stdin.readline()
            if not header:
                return
            cmd, rid, n = header.decode().split()
            body = stdin.read(int(n)).decode("utf-8")
        except KeyboardInterrupt:
            continue
        if cmd == "RUN":
            run(int(rid), body, ns)
        elif cmd == "SAVE":
            try:
                result = ns.get("result")
                if not isinstance(result, pd.DataFrame):
                    raise ValueError("there is no data frame named `result` in the console")
                result.to_parquet(body, index=False)
                reply({"id": int(rid), "ok": True})
            except BaseException as e:
                reply({"id": int(rid), "ok": False, "error": {"message": str(e), "line": None}})
        elif cmd == "EXIT":
            return


main()
