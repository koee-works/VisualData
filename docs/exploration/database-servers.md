# Exploration: database servers as sources

- **Status:** Exploration (2026-11-03), on branch `explore/db-servers`. Nothing here is in the app yet.
- **Question:** how should an analysis read from PostgreSQL, MySQL, SQL Server and cloud warehouses, keeping what the app promises about files: reproducible runs, read-only access, no surprises with passwords?

## What was tried

A spike crate, `crates/vda-server`, reads a table or a query from a real PostgreSQL 16 server into the Parquet copy every source is held as. `tests/postgres.rs` runs it against a local server with a 500,000-row table, a table of awkward types, a superuser, and a role that may only SELECT (`tests/seed.sql` builds them). The tests run when `VDA_TEST_PG=host:port` is set and skip otherwise. `examples/scale.rs` measures time and memory for any number of rows.

### How a read works

1. **Prepare the query.** Nothing runs yet. This is one statement only (the extended protocol refuses two), and it gives the columns and their server types.
2. **Stream it out inside a read-only transaction.** `COPY (SELECT <conversions> FROM (<query>) q) TO STDOUT (FORMAT csv)` goes to a temporary file. The session is set to UTC, ISO dates, ISO 8601 intervals and exact floats.
3. **Convert to Parquet.** DuckDB reads the CSV with exact column types and writes Parquet.

The server converts its own types in step 2, because it knows them best:

| Server type | In the app | Note |
|---|---|---|
| boolean | true/false | |
| smallint, integer, bigint, oid | whole number | bigint exact to 2^63−1 |
| real, double precision | decimal | NaN and ±Infinity become missing |
| numeric | decimal | about 15 significant digits kept |
| money | decimal | without the currency |
| date, timestamp | date, date and time | ±infinity becomes missing |
| timestamptz | date and time | converted to UTC |
| text, varchar, char, name, enums | text | |
| uuid, json, jsonb, arrays, interval, time, inet, … | text | intervals as ISO 8601 (`P1DT2H`) |
| bytea | text | shown by size, `<2 bytes>` |

### What the tests show

- **Reading a table:** 500,000 rows (7 columns) took **1.5 s** end to end on 2 cores, including Parquet. The server streamed 21 MB, and the Parquet file was 2.2 MB.
- **Memory stays flat** once DuckDB's conversion is capped (256 MB). A generated query of four columns:

  | Rows | Time | Streamed | Parquet | Peak memory |
  |---|---|---|---|---|
  | 500,000 | 0.9 s | 28 MB | 18 MB | 136 MB |
  | 5,000,000 | 8.0 s | 290 MB | 185 MB | 272 MB |
  | 15,000,000 | 23.8 s | 877 MB | 555 MB | 273 MB |

  Without the cap, 5 million rows took 412 MB, because DuckDB uses whatever memory it is allowed. The fetch itself streams through a fixed 64 KB buffer.
- **The fingerprint is stable.** Two reads of the 500,000-row table give the same SHA-256. Changing one value changes it, and changing it back restores the original. Table reads are ordered by the primary key, so rows rewritten elsewhere on disk still stream in the same order.
- **Nothing can change the database, even as a superuser.**
  - `DELETE … RETURNING` and data-modifying `WITH` cannot even be parsed, because the query is read as a subquery. The message says the query must be a SELECT.
  - `SELECT … FOR UPDATE`, `lo_import()` and `nextval()` are refused by the read-only transaction.
  - Two statements are refused by the protocol.
  - The table still had all its rows afterwards.
- **The server's permissions decide what a user reads:** "permission denied for table salaries".
- **A wrong password says so:** `password authentication failed for user "reader"`.
- **Timeouts are enforced by the server** (`statement_timeout`, set per connection).
- **Cancel** sends the server's cancel request and stops a 20-second query within a second. The connection stays usable afterwards.

## Proposed design

### 1. Connections belong to the user; the project names them

A project should not carry hosts, user names or passwords. It is shared, and a colleague may reach the same data through another account or a different host.

- **On each computer:** the user defines named connections, for example `warehouse` (PostgreSQL, host, port, database, user, TLS mode). They are kept per user beside the approvals file (`connections.json`).
- **In the project:** a source refers to the connection by name, plus a table or a query. It also keeps a hint (kind, host, database) so that someone opening the project is offered a connection with those details already filled in.

### 2. Passwords never touch a project file

| Where the password comes from | When |
|---|---|
| The operating system's keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service) | The default in the desktop app (`keyring` crate) |
| `VDA_CONNECTION_<NAME>_PASSWORD` | For `vda-run` on servers and in schedules, where no keychain is unlocked |
| `~/.pgpass` (and the MySQL equivalent) | Already how many analysts store them |
| Nothing | Servers that trust the network or use client certificates |

What can't be done yet: `rust-postgres` has no Kerberos/GSSAPI. Cloud identity tokens (AWS RDS IAM, Azure AD) would have to be fetched and passed as the password, which is possible but is separate work.

### 3. A server source is a snapshot you refresh

A file has a content hash; a server table does not. So a server source holds what was read last time:

- **Runs use the snapshot.** They never contact the server, so they are fast, work offline, and give the same answer twice.
- **Refresh from server** in the app, and `vda-run` by default, read it again.
- **The fingerprint is the hash of the rows as they stream** (implemented and tested in the spike). If nothing changed on the server, the hash is the same and nothing is outdated. If rows changed, exactly the steps that read the source are outdated, as with a changed file. The run record keeps when the snapshot was taken and its hash.
- **Order must be stable for the hash to mean anything.** A table read orders by its primary key. A query without `ORDER BY` gets a warning, because servers may return rows in a different order each time.

### 4. Drivers

| Database | Route | Notes |
|---|---|---|
| PostgreSQL (and Redshift, Supabase, Neon, Timescale, CockroachDB) | `postgres` crate, as in the spike | Pure Rust protocol; TLS through `postgres-native-tls` or rustls |
| MySQL, MariaDB | `mysql` crate | Same plan: prepare for columns; read-only with `START TRANSACTION READ ONLY` and a server-side time limit; streamed rows |
| SQL Server | `tiberius` | Async, wrapped like the `postgres` crate wraps `tokio-postgres`; read-only by snapshot isolation plus a read-only login (SQL Server has no read-only transaction) |
| Snowflake, BigQuery, Databricks | ADBC drivers (Arrow Database Connectivity), later | They return Arrow, which DuckDB writes straight to Parquet; each needs its own sign-in (key pair, OAuth) |
| Anything else | ODBC (`odbc-api`), as a fallback | Needs the vendor's driver installed by the user |

DuckDB's own `postgres` and `mysql` extensions were considered and set aside for now:
- They are downloaded at run time, which ADR-0001 rules out (and the download host is unreachable here).
- Bundling them means building DuckDB with them compiled in.
- Their speed (parallel reads, filters pushed to the server) matters less once a source is a snapshot that runs read locally.

### 5. Read-only, everywhere

The guarantee is that the app never changes a database, whatever the account can do:
- The query is read as a subquery.
- It runs inside a read-only transaction (or the database's nearest equivalent).
- One statement only.
- A server-side time limit.

The app should still recommend a read-only account, and say when a connection's user can write.

## Open questions for you

1. **Which databases do your analysts use?** This decides the order after PostgreSQL. MySQL is a day's work on the same pattern; SQL Server and the warehouses are more.
2. **Snapshots, or live reads?** The design above refreshes on demand or on a schedule. Some teams want every run to read the server.
3. **Sign-in:** are passwords enough for now, or do you need single sign-on (Kerberos, Azure AD, Okta) from the start?
4. **Tunnels:** do your servers sit behind SSH bastions or VPNs the app should reach through?

## Next steps if this goes ahead

1. Move the spike into the app: connections, the keychain, server sources as snapshots with Refresh, the UI, and `vda-run --refresh-sources`.
2. Add TLS (`require` and `verify-full`), tested against a server with a certificate.
3. Add MySQL on the same pattern, tested against a real server in CI (CI service containers make this easy).
4. Then packaging, which also has to ship `vda-run`.
