-- The test database: run as a superuser named `analyst` (password
-- `secret-pw`) on an empty server, then `CREATE DATABASE shop` and run this
-- file in it:  psql -h 127.0.0.1 -U analyst -d shop -f seed.sql
CREATE TABLE orders AS
SELECT i AS order_id,
       DATE '2026-01-01' + (i % 300) AS order_date,
       (ARRAY['East','West','North','South'])[1 + i % 4] AS region,
       (ARRAY['Basic','Plus','Pro'])[1 + i % 3] AS product,
       1 + i % 9 AS units,
       round((12 + (i % 50) * 0.75)::numeric, 2) AS unit_price,
       CASE WHEN i % 97 = 0 THEN NULL ELSE round(((1 + i % 9) * (12 + (i % 50) * 0.75))::numeric, 2) END AS revenue
FROM generate_series(1, 500000) i;
ALTER TABLE orders ADD PRIMARY KEY (order_id);
CREATE TABLE odd_types (
  id serial PRIMARY KEY, flag boolean, small smallint, big bigint, real_n real, dbl double precision,
  exact numeric(38,10), money_amt money, label varchar(20), note text, day date,
  at_local timestamp, at_tz timestamptz, clock time, span interval, uid uuid, doc jsonb, tags text[], raw bytea, ip inet);
INSERT INTO odd_types (flag, small, big, real_n, dbl, exact, money_amt, label, note, day, at_local, at_tz, clock, span, uid, doc, tags, raw, ip) VALUES
 (true, 7, 9223372036854775807, 1.5, 2.25, 12345678901234567890.0123456789, 19.99, 'a', 'O''Hare', '2026-07-01', '2026-07-01 10:30:00', '2026-07-01 10:30:00+02', '10:30', '1 day 2 hours', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', '{"k": [1, 2]}', ARRAY['x','y'], '\x0102', '10.0.0.1'),
 (NULL, NULL, NULL, 'NaN', 'Infinity', NULL, NULL, NULL, NULL, '1969-12-31', NULL, '1969-12-31 23:59:59.5+00', NULL, NULL, NULL, 'null', '{}', NULL, NULL);
CREATE ROLE reader LOGIN PASSWORD 'reader-pw';
GRANT SELECT ON ALL TABLES IN SCHEMA public TO reader;
CREATE VIEW east AS SELECT * FROM orders WHERE region = 'East';
GRANT SELECT ON east TO reader;
