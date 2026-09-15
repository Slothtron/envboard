# Changelog

All notable changes to `slothtron-envboard` are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - unreleased

### Added
- Multi-environment registry: name + DNS servers + domain suffix + static host
  overrides + colour/description, with CRUD and an active-environment switch.
  Persisted atomically to `<confdir>/envboard.json` with mode `0600`.
- Dynamic DNS server discovery, three sources: the environment's own
  `dns_servers`, runtime edits through the dashboard, and
  `mitmproxy_rs.dns.get_system_dns_servers()` for the OS configuration.
- Forward resolution (host → ip) via a per-environment
  `mitmproxy_rs.dns.DnsResolver` (Rust / hickory).
- Multi-environment comparison: resolve the same hostnames against every
  environment's DNS servers in one call (`all_envs`).
- Passive collection of host/ip mappings from `dns_response` when running with
  `--mode dns`.
- Bidirectional mapping index with per-environment sharding, TTLs, provenance
  (`static` / `passive` / `active`) and source precedence. Forward only —
  there is no ip → host reverse lookup by design.
- Flow annotation (`flow.comment` and/or `flow.metadata`) for hosts that match a
  known environment.
- Dashboard served from mitmweb at `/envboard/`, inheriting mitmweb's
  authentication, `Sec-Fetch-Site` guard and XSRF cookie handling. No second web
  server, no second auth system.
- Rules files: import a hand-written hosts-style file (`ip host…` or the reverse
  `host… ip`, several hosts sharing one IP, comments, blanks) and get a
  deterministic normalized rules file. Invalid content is ignored per item and
  reported (line + reason), never fatal; conflicting hosts resolve last-wins.
  One rules file is **bound per environment**, so switching environment switches
  the rules file. `Environment.hosts` still wins over the bound rules file.
- `examples/hosts.sample.txt`, a sanitized import sample (the real
  `examples/hosts.txt` is gitignored — it carries internal IPs and hostnames).
- 16 `envboard.*` mitmproxy commands giving full CLI/console parity with the
  dashboard (5 new: `rules.list` / `rules.import` / `rules.show` / `rules.bind`
  / `rules.remove`).
- `ci/verify.sh` verification entrypoint: compile-check, dependency-lint,
  unit tests, contract golden fixtures, best-effort mypy, pack-check and a smoke
  test. Reachable as `npm run verify`.
- `scripts/verify_live.sh` end-to-end check against a real mitmweb instance.

### Fixed
- `Environment.hosts` keys were stored **un-normalized**, so `API.Example.COM.`
  or `*.wild.example.com` never matched the normalized lookup host — a silent
  no-op, and contrary to what `core/spec/capabilities.md` promised. Hosts keys
  are now normalized on construction (`*.` prefix stripped, per the contract),
  and two keys collapsing to the same host is now a loud `invalid_config`.
- The dashboard's JavaScript was inline in `index.html`, which mitmweb's CSP
  (`default-src 'self'`, no `script-src`) makes browsers **refuse to execute**.
  The page still returned 200 and rendered, so the curl-based live check passed
  while the dashboard was in fact inert in a real browser (no data loaded, no
  button worked). The script now lives in `web/app.js`, served by a same-origin
  `AssetHandler`; `pack_check.py` rejects any inline `<script>` and
  `verify_live.sh` asserts the external asset is served.

### Known limitations
- `mypy` is not vendored; `typecheck` degrades to a skip with an explicit notice
  when it is unavailable.
- `envboard.resolve` is an asynchronous command implemented as "schedule + read
  the mapping table", because mitmproxy has no awaitable command path. The REST
  endpoints await properly and return results directly.
- Environment switching is observation-only (L1). Traffic rewriting (L2) is
  deliberately out of scope for this version.
