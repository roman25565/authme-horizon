#!/usr/bin/env python3
"""Imports AuthMeReloaded's SQLite accounts (plugins/AuthMe/authme.db) into AuthMe for Horizon's
config/authme/accounts.json. Only Python's standard library.

    python import_authme_db.py <authme.db> <server folder>

Accounts already in accounts.json are kept (an imported name never overwrites one). Hashes are
copied as they are: AuthMe's SHA256 ($SHA$salt$hash) is verified by the mod and replaced with
scrypt at that player's next login. Other AuthMe hash formats are skipped and listed.
Stop the server first, or run `/authme reload` after importing.
"""
import json, os, sqlite3, sys

def main():
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    db, server = sys.argv[1], sys.argv[2]
    out_dir = os.path.join(server, "config", "authme")
    out = os.path.join(out_dir, "accounts.json")
    os.makedirs(out_dir, exist_ok=True)
    data = {"version": 1, "accounts": {}}
    if os.path.exists(out):
        with open(out, encoding="utf-8") as f:
            data = json.load(f)
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    con.row_factory = sqlite3.Row
    table = "authme"
    cols = {r[1] for r in con.execute(f"PRAGMA table_info({table})")}
    rows = con.execute(f"SELECT * FROM {table}").fetchall()
    added, kept, skipped = 0, 0, []
    for r in rows:
        name = (r["realname"] if "realname" in cols and r["realname"] else r["username"]) or ""
        pw = r["password"] or ""
        if not name or not all(c.isascii() and (c.isalnum() or c == "_") for c in name) or len(name) > 16:
            skipped.append(f"{name!r}: name")
            continue
        if not pw.startswith("$SHA$"):
            skipped.append(f"{name}: hash format not supported")
            continue
        key = name.lower()
        if key in data["accounts"]:
            kept += 1
            continue
        def secs(col):
            v = r[col] if col in cols else None
            return int(v) // 1000 if v else None
        acc = {"realname": name, "hash": pw, "registered": secs("regdate") or 0,
               "last_login": secs("lastlogin"), "email": None}
        email = r["email"] if "email" in cols else None
        if email and email != "your@email.com":
            acc["email"] = email
        data["accounts"][key] = acc
        added += 1
    tmp = out + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, indent=2, ensure_ascii=False)
    os.replace(tmp, out)
    print(f"Imported {added}, kept {kept} existing, skipped {len(skipped)} -> {out}")
    for s in skipped:
        print("  skipped", s)
    return 0

if __name__ == "__main__":
    sys.exit(main())
