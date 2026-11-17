# AuthMe for Horizon

A server-side login mod for [Horizon](https://github.com/horizon-rust/horizon) (Minecraft 26.3 in Rust):
a port of the **[AuthMeReloaded](https://github.com/AuthMe/AuthMeReloaded)** plugin as a native Rust mod (`.hnmod`).

> **Credits and license.** This is a derivative work of AuthMeReloaded (authors: the AuthMe team, Xephi, sgdc3,
> ljacqu and other contributors), so the mod is distributed under the **GNU GPL v3** (see `LICENSE`).
> The Ukrainian message texts come from AuthMeReloaded's `messages_uk.yml`. The code is written from scratch in Rust:
> AuthMe's Java code does not run in Horizon.

## Features

| Command | Who | What it does |
|---|---|---|
| `/register <password> <password>` (`/reg`) | player | register |
| `/login <password>` (`/l`) | player | log in |
| `/logout` | player | log out |
| `/changepassword <old> <new>` | player | change password |
| `/unregister <password>` | player | delete your account |
| `/authme register\|unregister\|password\|forcelogin\|lastlogin\|list\|reload\|version` | operator (level 3) and console | admin commands |

Until a player logs in:
- they are put into spectator mode (no inventory, dropping, picking up or damage) and blinded; after login
  their previous game mode is restored (even after a server crash, it is saved in `limbo.json`);
- they stay in place (pulled back if they move), can't chat and can't run other commands;
- clicking blocks and entities, attacking, opening chests, picking up and the Q key are blocked;
- after 180 s without logging in they are kicked; 3 wrong passwords in a row also kick;
- a nickname with invalid characters or with different letter case than the registered one (`Roman` vs `roman`) is refused.
- **sessions**: a player who joins with the same nickname from the same IP address as their last password login, within `session_minutes` minutes (default 4320, i.e. 3 days) of the last login, gets in without a password, and the timer starts again. A different IP means a password. `/logout` and a password change by an admin end the session. `session_minutes = 0` disables sessions. On a shared address (one PC, shared NAT or a test on 127.0.0.1) anyone with that nickname also gets in without a password, as in the original AuthMe.

Passwords: **scrypt** (N=2^15, r=8, p=3: the OWASP-recommended setting, 32 MB of memory per hash), computed on separate
threads so the server doesn't stall. Argon2 and bcrypt are not possible: Horizon only lets a mod use libraries already in
its `Cargo.lock`, and those aren't there; scrypt is in the same class (resistant to GPU brute force). Old AuthMe `SHA256`
hashes work after import and are silently replaced with scrypt on first login.

## Files

```
authme_horizon/              the mod itself (bundle): horizon-mixin.toml, api/ (Rust code), gate.rs, chat.rs, drop.rs, admit.rs
dist/authme-0.1.0.hnmod      the same mod as one file
core-tests/                  logic tests: cargo test
tools/import_authme_db.py    import accounts from an old plugins/AuthMe/authme.db
tools/pack-hnmod.py          rebuild the .hnmod
test-server/setup.ps1        build a server engine with the mod and create a test server
```

Settings: `config/authme/config.toml` in the server folder (created on first start), accounts in
`config/authme/accounts.json`. After changes: `/authme reload`.

Message language: `language = "en"` (English, default) or `language = "uk"` (Ukrainian). An unknown code
means English. To add a language you need a variant in `Language` and a table of texts in `api/src/core/messages.rs`.

## Running

1. PowerShell: `powershell -ExecutionPolicy Bypass -File .\test-server\setup.ps1`
   (builds `horizon-engine.exe` with the mod via `horizon-launcher native-build ... --server-output`, creates an offline server
   on port 25566 in `test-server\server` and points it to that engine).
2. `horizon-server --dir .\test-server\server run`, join `localhost:25566`.
3. Migrate old accounts: `python tools\import_authme_db.py <path to plugins\AuthMe\authme.db> <server folder>`.

## Limitations (what Horizon doesn't give a mod yet)

- The hook catalogue doesn't provide the player's IP; the mod gets it with a separate injection into `Shared::try_admit` (the network admission of a player) and uses it only for sessions. Registration limits per IP, `/authme accounts` and `getip` are not implemented.
- **Email (`/email`), captcha, 2FA (TOTP), antibot, MySQL** are not ported.
- If someone joins with a nickname that is already online, Horizon itself kicks the player who was online (before the mod sees it).
  That doesn't give the attacker the password, but they can kick the player.
- The mod is pinned to Horizon commit `2944480 (nightly-2026-11-03-2944480)` because chat and the Q key are intercepted by injections into game functions,
  not through stable hooks. After updating Horizon, check and update `horizon_revision`.

## License

**GNU GPL v3** (see [LICENSE](LICENSE)), a derivative work of [AuthMeReloaded](https://github.com/AuthMe/AuthMeReloaded) (credits above). A mod for [Horizon](https://github.com/horizon-rust/horizon) (MIT, Ludovic Marie and the Horizon contributors).

Unofficial project, not affiliated with Mojang Studios or Microsoft. Minecraft is a trademark of Mojang Synergies AB.
