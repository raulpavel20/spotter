#!/usr/bin/env bash
# Builds the demo repository for record-demo.sh: a small TypeScript billing
# API with a feature branch an agent has been working on. The older commits
# are already reviewed, one is half reviewed, and the agent has uncommitted
# work in progress.
#   assets/demo-project.sh <dir>
set -euo pipefail
DIR=$1
mkdir -p "$DIR"
cd "$DIR"
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
NOW=$(date +%s)

# commit <minutes ago> <message>: commits everything, dated in the past.
commit() {
  local when="@$((NOW - $1 * 60)) +0000"
  git add -A
  GIT_AUTHOR_DATE=$when GIT_COMMITTER_DATE=$when git commit -qm "$2"
}

git init -q -b main
git config user.name "Agent"
git config user.email agent@example.com
mkdir -p src/db src/lib src/routes src/payments test migrations

# === main: the API before the feature =====================================

cat > package.json <<'EOF'
{
  "name": "billing-api",
  "version": "0.4.0",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "tsx watch src/server.ts",
    "build": "tsc -p .",
    "test": "vitest run"
  },
  "dependencies": {
    "fastify": "^4.28.1",
    "pg": "^8.12.0",
    "zod": "^3.23.8"
  },
  "devDependencies": {
    "@types/pg": "^8.11.6",
    "tsx": "^4.16.2",
    "typescript": "^5.5.4",
    "vitest": "^2.0.5"
  }
}
EOF

cat > tsconfig.json <<'EOF'
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "outDir": "dist"
  },
  "include": ["src"]
}
EOF

cat > README.md <<'EOF'
# billing-api

Invoices for the shop, served over HTTP.

    npm install
    DATABASE_URL=postgres://localhost/billing npm run dev

| Route | |
|---|---|
| `GET /invoices?customer=<id>` | A customer's invoices |
| `GET /invoices/<id>` | One invoice |
EOF

cat > migrations/001_init.sql <<'EOF'
CREATE TABLE customers (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  email text NOT NULL UNIQUE,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE invoices (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  customer_id uuid NOT NULL REFERENCES customers(id),
  status text NOT NULL CHECK (status IN ('draft', 'open', 'void')),
  total_minor bigint NOT NULL,
  currency char(3) NOT NULL,
  due_at timestamptz NOT NULL
);

CREATE INDEX invoices_customer_idx ON invoices (customer_id, due_at DESC);
EOF

cat > src/config.ts <<'EOF'
import { z } from "zod";

const Env = z.object({
  PORT: z.coerce.number().default(3000),
  DATABASE_URL: z.string().url(),
  LOG_LEVEL: z.enum(["debug", "info", "warn", "error"]).default("info"),
});

export type Config = z.infer<typeof Env>;

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const parsed = Env.safeParse(env);
  if (!parsed.success) {
    const issues = parsed.error.issues.map(
      (i) => `${i.path.join(".")}: ${i.message}`,
    );
    throw new Error(`invalid configuration:\n  ${issues.join("\n  ")}`);
  }
  return parsed.data;
}
EOF

cat > src/server.ts <<'EOF'
import Fastify from "fastify";

import { loadConfig } from "./config.js";
import { createPool } from "./db/client.js";
import { logger } from "./lib/logger.js";
import { healthRoutes } from "./routes/health.js";
import { invoiceRoutes } from "./routes/invoices.js";

const config = loadConfig();
const db = createPool(config.DATABASE_URL);
const app = Fastify({ logger: false });

app.register(healthRoutes, { db });
app.register(invoiceRoutes, { db, prefix: "/invoices" });

app.listen({ port: config.PORT, host: "0.0.0.0" }).then(
  (address) => logger.info("listening", { address }),
  (err) => {
    logger.error("failed to start", { err: String(err) });
    process.exit(1);
  },
);
EOF

cat > src/db/client.ts <<'EOF'
import pg from "pg";

export type Db = pg.Pool;

export function createPool(url: string): Db {
  return new pg.Pool({ connectionString: url, max: 10 });
}

/** Runs `fn` in a transaction, rolling back if it throws. */
export async function inTransaction<T>(
  db: Db,
  fn: (client: pg.PoolClient) => Promise<T>,
): Promise<T> {
  const client = await db.connect();
  try {
    await client.query("BEGIN");
    const result = await fn(client);
    await client.query("COMMIT");
    return result;
  } catch (err) {
    await client.query("ROLLBACK");
    throw err;
  } finally {
    client.release();
  }
}
EOF

cat > src/db/invoices.ts <<'EOF'
import type { Db } from "./client.js";
import type { Money } from "../lib/money.js";

export type InvoiceStatus = "draft" | "open" | "void";

export interface Invoice {
  id: string;
  customerId: string;
  status: InvoiceStatus;
  total: Money;
  dueAt: Date;
}

interface Row {
  id: string;
  customer_id: string;
  status: InvoiceStatus;
  total_minor: string;
  currency: string;
  due_at: Date;
}

function fromRow(row: Row): Invoice {
  return {
    id: row.id,
    customerId: row.customer_id,
    status: row.status,
    total: { minor: BigInt(row.total_minor), currency: row.currency },
    dueAt: row.due_at,
  };
}

export async function findInvoice(
  db: Db,
  id: string,
): Promise<Invoice | null> {
  const { rows } = await db.query<Row>(
    "SELECT * FROM invoices WHERE id = $1",
    [id],
  );
  return rows[0] ? fromRow(rows[0]) : null;
}

export async function listInvoices(
  db: Db,
  customerId: string,
): Promise<Invoice[]> {
  const { rows } = await db.query<Row>(
    "SELECT * FROM invoices WHERE customer_id = $1 ORDER BY due_at DESC",
    [customerId],
  );
  return rows.map(fromRow);
}
EOF

cat > src/lib/money.ts <<'EOF'
export interface Money {
  /** The amount in the currency's minor unit, such as cents. */
  minor: bigint;
  currency: string;
}

export function add(a: Money, b: Money): Money {
  if (a.currency !== b.currency) {
    throw new Error(`currency mismatch: ${a.currency} vs ${b.currency}`);
  }
  return { minor: a.minor + b.minor, currency: a.currency };
}

export function format(m: Money): string {
  const major = Number(m.minor) / 100;
  const fmt = new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: m.currency,
  });
  return fmt.format(major);
}
EOF

cat > src/lib/logger.ts <<'EOF'
type Level = "debug" | "info" | "warn" | "error";
type Fields = Record<string, unknown>;

const order: Record<Level, number> = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
};
const threshold = (process.env.LOG_LEVEL as Level | undefined) ?? "info";

function write(level: Level, msg: string, fields: Fields = {}) {
  if (order[level] < order[threshold]) return;
  const line = { time: new Date().toISOString(), level, msg, ...fields };
  process.stdout.write(JSON.stringify(line) + "\n");
}

export const logger = {
  debug: (msg: string, fields?: Fields) => write("debug", msg, fields),
  info: (msg: string, fields?: Fields) => write("info", msg, fields),
  warn: (msg: string, fields?: Fields) => write("warn", msg, fields),
  error: (msg: string, fields?: Fields) => write("error", msg, fields),
};
EOF

cat > src/lib/errors.ts <<'EOF'
/** An error that maps to an HTTP response. */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export class BadRequest extends HttpError {
  constructor(message: string) {
    super(400, message);
  }
}

export class NotFound extends HttpError {
  constructor(what: string) {
    super(404, `${what} not found`);
  }
}
EOF

cat > src/routes/health.ts <<'EOF'
import type { FastifyPluginAsync } from "fastify";

import type { Db } from "../db/client.js";

export const healthRoutes: FastifyPluginAsync<{ db: Db }> = async (
  app,
  { db },
) => {
  app.get("/health", async () => {
    await db.query("SELECT 1");
    return { ok: true };
  });
};
EOF

cat > src/routes/invoices.ts <<'EOF'
import type { FastifyPluginAsync } from "fastify";
import { z } from "zod";

import type { Db } from "../db/client.js";
import { findInvoice, listInvoices } from "../db/invoices.js";
import { NotFound } from "../lib/errors.js";
import { format } from "../lib/money.js";

const Params = z.object({ id: z.string().uuid() });
const Query = z.object({ customer: z.string().uuid() });

export const invoiceRoutes: FastifyPluginAsync<{ db: Db }> = async (
  app,
  { db },
) => {
  app.get("/", async (req) => {
    const { customer } = Query.parse(req.query);
    const invoices = await listInvoices(db, customer);
    return invoices.map((inv) => ({ ...inv, total: format(inv.total) }));
  });

  app.get("/:id", async (req) => {
    const { id } = Params.parse(req.params);
    const invoice = await findInvoice(db, id);
    if (!invoice) throw new NotFound(`invoice ${id}`);
    return { ...invoice, total: format(invoice.total) };
  });
};
EOF

cat > test/money.test.ts <<'EOF'
import { describe, expect, it } from "vitest";

import { add, format } from "../src/lib/money.js";

describe("money", () => {
  it("adds amounts in the same currency", () => {
    const a = { minor: 150n, currency: "USD" };
    const b = { minor: 250n, currency: "USD" };
    expect(add(a, b)).toEqual({ minor: 400n, currency: "USD" });
  });

  it("refuses to mix currencies", () => {
    const usd = { minor: 1n, currency: "USD" };
    const eur = { minor: 1n, currency: "EUR" };
    expect(() => add(usd, eur)).toThrow(/mismatch/);
  });

  it("formats dollars", () => {
    expect(format({ minor: 123456n, currency: "USD" })).toBe("$1,234.56");
  });
});
EOF
commit 4000 "Initial billing API"
git checkout -qb feature/payments

# === 1 ======================================================================

cat > src/payments/provider.ts <<'EOF'
import type { Money } from "../lib/money.js";

export type PaymentStatus = "pending" | "succeeded" | "failed";

export interface Charge {
  id: string;
  status: PaymentStatus;
  amount: Money;
  failureReason?: string;
}

export interface ChargeRequest {
  amount: Money;
  /** A saved card or bank account. */
  methodId: string;
  /** Charging twice with the same key returns the first charge. */
  idempotencyKey: string;
}

export interface PaymentProvider {
  charge(req: ChargeRequest): Promise<Charge>;
  refund(chargeId: string, amount?: Money): Promise<Charge>;
}

export class ProviderError extends Error {
  constructor(
    message: string,
    readonly retryable: boolean,
  ) {
    super(message);
  }
}
EOF

cat > src/payments/fake.ts <<'EOF'
import { randomUUID } from "node:crypto";

import type { Charge, ChargeRequest, PaymentProvider } from "./provider.js";

/** An in-memory provider for development and tests. */
export class FakeProvider implements PaymentProvider {
  readonly charges = new Map<string, Charge>();
  private byKey = new Map<string, string>();

  async charge(req: ChargeRequest): Promise<Charge> {
    const seen = this.byKey.get(req.idempotencyKey);
    if (seen) return this.charges.get(seen)!;

    const declined = req.methodId.startsWith("pm_decline");
    const charge: Charge = {
      id: `ch_${randomUUID()}`,
      status: declined ? "failed" : "succeeded",
      amount: req.amount,
      failureReason: declined ? "card_declined" : undefined,
    };
    this.charges.set(charge.id, charge);
    this.byKey.set(req.idempotencyKey, charge.id);
    return charge;
  }

  async refund(chargeId: string): Promise<Charge> {
    const charge = this.charges.get(chargeId);
    if (!charge) throw new Error(`no such charge: ${chargeId}`);
    return { ...charge, status: "pending" };
  }
}
EOF

cat > src/config.ts <<'EOF'
import { z } from "zod";

const Env = z
  .object({
    PORT: z.coerce.number().default(3000),
    DATABASE_URL: z.string().url(),
    LOG_LEVEL: z.enum(["debug", "info", "warn", "error"]).default("info"),
    PAYMENTS_PROVIDER: z.enum(["fake", "live"]).default("fake"),
    PAYMENTS_API_KEY: z.string().optional(),
  })
  .refine((env) => env.PAYMENTS_PROVIDER === "fake" || env.PAYMENTS_API_KEY, {
    message: "required when PAYMENTS_PROVIDER=live",
    path: ["PAYMENTS_API_KEY"],
  });

export type Config = z.infer<typeof Env>;

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const parsed = Env.safeParse(env);
  if (!parsed.success) {
    const issues = parsed.error.issues.map(
      (i) => `${i.path.join(".")}: ${i.message}`,
    );
    throw new Error(`invalid configuration:\n  ${issues.join("\n  ")}`);
  }
  return parsed.data;
}
EOF
commit 142 "Add a payment provider interface"

# === 2 ======================================================================

cat > migrations/002_payments.sql <<'EOF'
ALTER TABLE invoices DROP CONSTRAINT invoices_status_check;
ALTER TABLE invoices ADD CONSTRAINT invoices_status_check
  CHECK (status IN ('draft', 'open', 'paid', 'void'));

CREATE TABLE payments (
  id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
  invoice_id uuid NOT NULL REFERENCES invoices(id),
  charge_id text NOT NULL UNIQUE,
  status text NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
  amount_minor bigint NOT NULL,
  currency char(3) NOT NULL,
  failure_reason text,
  created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX payments_invoice_idx ON payments (invoice_id, created_at DESC);
EOF

cat > src/db/payments.ts <<'EOF'
import type pg from "pg";

import type { Charge, PaymentStatus } from "../payments/provider.js";
import type { Db } from "./client.js";

export interface Payment {
  id: string;
  invoiceId: string;
  chargeId: string;
  status: PaymentStatus;
  failureReason: string | null;
  createdAt: Date;
}

export async function recordPayment(
  client: pg.PoolClient,
  invoiceId: string,
  charge: Charge,
): Promise<Payment> {
  const { rows } = await client.query(
    `INSERT INTO payments
       (invoice_id, charge_id, status, amount_minor, currency, failure_reason)
     VALUES ($1, $2, $3, $4, $5, $6)
     RETURNING id, created_at`,
    [
      invoiceId,
      charge.id,
      charge.status,
      charge.amount.minor.toString(),
      charge.amount.currency,
      charge.failureReason ?? null,
    ],
  );
  return {
    id: rows[0].id,
    invoiceId,
    chargeId: charge.id,
    status: charge.status,
    failureReason: charge.failureReason ?? null,
    createdAt: rows[0].created_at,
  };
}

export async function paymentsFor(
  db: Db,
  invoiceId: string,
): Promise<Payment[]> {
  const { rows } = await db.query(
    "SELECT * FROM payments WHERE invoice_id = $1 ORDER BY created_at DESC",
    [invoiceId],
  );
  return rows.map((r) => ({
    id: r.id,
    invoiceId: r.invoice_id,
    chargeId: r.charge_id,
    status: r.status,
    failureReason: r.failure_reason,
    createdAt: r.created_at,
  }));
}
EOF

cat > src/db/invoices.ts <<'EOF'
import type pg from "pg";

import type { Db } from "./client.js";
import type { Money } from "../lib/money.js";

export type InvoiceStatus = "draft" | "open" | "paid" | "void";

export interface Invoice {
  id: string;
  customerId: string;
  status: InvoiceStatus;
  total: Money;
  dueAt: Date;
}

interface Row {
  id: string;
  customer_id: string;
  status: InvoiceStatus;
  total_minor: string;
  currency: string;
  due_at: Date;
}

function fromRow(row: Row): Invoice {
  return {
    id: row.id,
    customerId: row.customer_id,
    status: row.status,
    total: { minor: BigInt(row.total_minor), currency: row.currency },
    dueAt: row.due_at,
  };
}

export async function findInvoice(
  db: Db,
  id: string,
): Promise<Invoice | null> {
  const { rows } = await db.query<Row>(
    "SELECT * FROM invoices WHERE id = $1",
    [id],
  );
  return rows[0] ? fromRow(rows[0]) : null;
}

export async function listInvoices(
  db: Db,
  customerId: string,
): Promise<Invoice[]> {
  const { rows } = await db.query<Row>(
    "SELECT * FROM invoices WHERE customer_id = $1 ORDER BY due_at DESC",
    [customerId],
  );
  return rows.map(fromRow);
}

/** Marks an open invoice paid. Throws if it was not open. */
export async function markPaid(
  client: pg.PoolClient,
  id: string,
): Promise<void> {
  const { rowCount } = await client.query(
    "UPDATE invoices SET status = 'paid' WHERE id = $1 AND status = 'open'",
    [id],
  );
  if (rowCount !== 1) throw new Error(`invoice ${id} is not open`);
}
EOF
commit 126 "Record payments against invoices"

# === 3 ======================================================================

cat > src/routes/payments.ts <<'EOF'
import type { FastifyPluginAsync } from "fastify";
import { z } from "zod";

import { inTransaction, type Db } from "../db/client.js";
import { findInvoice, markPaid } from "../db/invoices.js";
import { recordPayment } from "../db/payments.js";
import { Conflict, NotFound, PaymentFailed } from "../lib/errors.js";
import { logger } from "../lib/logger.js";
import type { PaymentProvider } from "../payments/provider.js";

const Params = z.object({ id: z.string().uuid() });
const Body = z.object({ methodId: z.string().min(1) });

interface Options {
  db: Db;
  provider: PaymentProvider;
}

export const paymentRoutes: FastifyPluginAsync<Options> = async (
  app,
  { db, provider },
) => {
  app.post("/:id/pay", async (req, reply) => {
    const { id } = Params.parse(req.params);
    const { methodId } = Body.parse(req.body);

    const invoice = await findInvoice(db, id);
    if (!invoice) throw new NotFound(`invoice ${id}`);
    if (invoice.status !== "open") {
      throw new Conflict(`invoice ${id} is ${invoice.status}`);
    }

    const charge = await provider.charge({
      amount: invoice.total,
      methodId,
      idempotencyKey: `invoice-${id}`,
    });

    const payment = await inTransaction(db, async (client) => {
      const payment = await recordPayment(client, id, charge);
      if (charge.status === "succeeded") await markPaid(client, id);
      return payment;
    });

    if (charge.status === "failed") {
      logger.warn("charge failed", { invoice: id, reason: charge.failureReason });
      throw new PaymentFailed(charge.failureReason ?? "unknown");
    }
    logger.info("invoice paid", { invoice: id, charge: charge.id });
    return reply.code(201).send(payment);
  });
};
EOF

cat > src/lib/errors.ts <<'EOF'
/** An error that maps to an HTTP response. */
export class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export class BadRequest extends HttpError {
  constructor(message: string) {
    super(400, message);
  }
}

export class PaymentFailed extends HttpError {
  constructor(readonly reason: string) {
    super(402, `payment failed: ${reason}`);
  }
}

export class NotFound extends HttpError {
  constructor(what: string) {
    super(404, `${what} not found`);
  }
}

export class Conflict extends HttpError {
  constructor(message: string) {
    super(409, message);
  }
}
EOF

cat > src/server.ts <<'EOF'
import Fastify from "fastify";

import { loadConfig } from "./config.js";
import { createPool } from "./db/client.js";
import { logger } from "./lib/logger.js";
import { FakeProvider } from "./payments/fake.js";
import { healthRoutes } from "./routes/health.js";
import { invoiceRoutes } from "./routes/invoices.js";
import { paymentRoutes } from "./routes/payments.js";

const config = loadConfig();
const db = createPool(config.DATABASE_URL);
const app = Fastify({ logger: false });

if (config.PAYMENTS_PROVIDER !== "fake") {
  throw new Error("only the fake payments provider is wired up so far");
}
const provider = new FakeProvider();

app.register(healthRoutes, { db });
app.register(invoiceRoutes, { db, prefix: "/invoices" });
app.register(paymentRoutes, { db, provider, prefix: "/invoices" });

app.listen({ port: config.PORT, host: "0.0.0.0" }).then(
  (address) => logger.info("listening", { address }),
  (err) => {
    logger.error("failed to start", { err: String(err) });
    process.exit(1);
  },
);
EOF
commit 104 "Add POST /invoices/:id/pay"

# === 4 ======================================================================

cat > src/payments/webhooks.ts <<'EOF'
import { createHmac, timingSafeEqual } from "node:crypto";

/** How far a webhook's timestamp may drift from our clock. */
const TOLERANCE_SECONDS = 5 * 60;

export interface WebhookEvent {
  id: string;
  type: "charge.succeeded" | "charge.failed" | "charge.refunded";
  chargeId: string;
  createdAt: number;
}

export class SignatureError extends Error {}

/**
 * Checks a `t=<unix>,v1=<hex>` signature header against the raw body and
 * returns the event. Throws a SignatureError if the header is missing,
 * stale or wrong.
 */
export function verifyWebhook(
  rawBody: string,
  header: string | undefined,
  secret: string,
  now = Math.floor(Date.now() / 1000),
): WebhookEvent {
  if (!header) throw new SignatureError("missing signature header");
  const parts = Object.fromEntries(
    header.split(",").map((kv) => kv.split("=", 2)),
  );

  const timestamp = Number(parts.t);
  if (!Number.isInteger(timestamp)) {
    throw new SignatureError("malformed timestamp");
  }
  if (Math.abs(now - timestamp) > TOLERANCE_SECONDS) {
    throw new SignatureError("timestamp outside tolerance");
  }

  const expected = createHmac("sha256", secret)
    .update(`${timestamp}.${rawBody}`)
    .digest();
  const given = Buffer.from(parts.v1 ?? "", "hex");
  if (given.length !== expected.length || !timingSafeEqual(given, expected)) {
    throw new SignatureError("signature mismatch");
  }
  return JSON.parse(rawBody) as WebhookEvent;
}
EOF

cat > src/routes/webhooks.ts <<'EOF'
import type { FastifyPluginAsync } from "fastify";

import type { Db } from "../db/client.js";
import { setPaymentStatus } from "../db/payments.js";
import { logger } from "../lib/logger.js";
import {
  SignatureError,
  verifyWebhook,
  type WebhookEvent,
} from "../payments/webhooks.js";

interface Options {
  db: Db;
  /** Shared with the provider, which signs every webhook with it. */
  secret: string;
}

export const webhookRoutes: FastifyPluginAsync<Options> = async (
  app,
  { db, secret },
) => {
  // Signatures cover the exact bytes we were sent, so keep the body raw.
  app.addContentTypeParser(
    "application/json",
    { parseAs: "string" },
    (_req, body, done) => done(null, body),
  );

  app.post("/payments", async (req, reply) => {
    let event: WebhookEvent;
    try {
      const signature = req.headers["x-signature"] as string | undefined;
      event = verifyWebhook(req.body as string, signature, secret);
    } catch (err) {
      if (!(err instanceof SignatureError)) throw err;
      logger.warn("rejected webhook", { reason: err.message });
      return reply.code(400).send({ error: err.message });
    }

    switch (event.type) {
      case "charge.succeeded":
        await setPaymentStatus(db, event.chargeId, "succeeded");
        break;
      case "charge.failed":
        await setPaymentStatus(db, event.chargeId, "failed");
        break;
      default:
        logger.debug("ignored webhook", { type: event.type });
    }
    return reply.code(204).send();
  });
};
EOF

cat >> src/db/payments.ts <<'EOF'

/** Applies a status the provider reported after the fact. */
export async function setPaymentStatus(
  db: Db,
  chargeId: string,
  status: PaymentStatus,
): Promise<void> {
  await db.query("UPDATE payments SET status = $2 WHERE charge_id = $1", [
    chargeId,
    status,
  ]);
}
EOF

cat > src/config.ts <<'EOF'
import { z } from "zod";

const Env = z
  .object({
    PORT: z.coerce.number().default(3000),
    DATABASE_URL: z.string().url(),
    LOG_LEVEL: z.enum(["debug", "info", "warn", "error"]).default("info"),
    PAYMENTS_PROVIDER: z.enum(["fake", "live"]).default("fake"),
    PAYMENTS_API_KEY: z.string().optional(),
    WEBHOOK_SECRET: z.string().min(16),
  })
  .refine((env) => env.PAYMENTS_PROVIDER === "fake" || env.PAYMENTS_API_KEY, {
    message: "required when PAYMENTS_PROVIDER=live",
    path: ["PAYMENTS_API_KEY"],
  });

export type Config = z.infer<typeof Env>;

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const parsed = Env.safeParse(env);
  if (!parsed.success) {
    const issues = parsed.error.issues.map(
      (i) => `${i.path.join(".")}: ${i.message}`,
    );
    throw new Error(`invalid configuration:\n  ${issues.join("\n  ")}`);
  }
  return parsed.data;
}
EOF

cat > src/server.ts <<'EOF'
import Fastify from "fastify";

import { loadConfig } from "./config.js";
import { createPool } from "./db/client.js";
import { logger } from "./lib/logger.js";
import { FakeProvider } from "./payments/fake.js";
import { healthRoutes } from "./routes/health.js";
import { invoiceRoutes } from "./routes/invoices.js";
import { paymentRoutes } from "./routes/payments.js";
import { webhookRoutes } from "./routes/webhooks.js";

const config = loadConfig();
const db = createPool(config.DATABASE_URL);
const app = Fastify({ logger: false });

if (config.PAYMENTS_PROVIDER !== "fake") {
  throw new Error("only the fake payments provider is wired up so far");
}
const provider = new FakeProvider();

app.register(healthRoutes, { db });
app.register(invoiceRoutes, { db, prefix: "/invoices" });
app.register(paymentRoutes, { db, provider, prefix: "/invoices" });
app.register(webhookRoutes, {
  db,
  secret: config.WEBHOOK_SECRET,
  prefix: "/webhooks",
});

app.listen({ port: config.PORT, host: "0.0.0.0" }).then(
  (address) => logger.info("listening", { address }),
  (err) => {
    logger.error("failed to start", { err: String(err) });
    process.exit(1);
  },
);
EOF
commit 88 "Verify webhook signatures"

# === 5 ======================================================================

cat > migrations/003_refunds.sql <<'EOF'
ALTER TABLE payments DROP CONSTRAINT payments_status_check;
ALTER TABLE payments ADD CONSTRAINT payments_status_check
  CHECK (status IN ('pending', 'succeeded', 'failed', 'refunded'));
EOF

perl -pi -e 's/^export type PaymentStatus = .*/export type PaymentStatus = "pending" | "succeeded" | "failed" | "refunded";/' \
  src/payments/provider.ts

cat > src/routes/refunds.ts <<'EOF'
import type { FastifyPluginAsync } from "fastify";
import { z } from "zod";

import type { Db } from "../db/client.js";
import { findPayment, setPaymentStatus } from "../db/payments.js";
import { Conflict, NotFound } from "../lib/errors.js";
import { logger } from "../lib/logger.js";
import type { PaymentProvider } from "../payments/provider.js";

const Params = z.object({ id: z.string().uuid() });

interface Options {
  db: Db;
  provider: PaymentProvider;
}

export const refundRoutes: FastifyPluginAsync<Options> = async (
  app,
  { db, provider },
) => {
  app.post("/:id/refund", async (req, reply) => {
    const { id } = Params.parse(req.params);
    const payment = await findPayment(db, id);
    if (!payment) throw new NotFound(`payment ${id}`);
    if (payment.status !== "succeeded") {
      throw new Conflict(`payment ${id} is ${payment.status}`);
    }

    const refund = await provider.refund(payment.chargeId);
    await setPaymentStatus(db, payment.chargeId, "refunded");
    logger.info("payment refunded", { payment: id, charge: refund.id });
    return reply.code(202).send({ id, status: "refunded" });
  });
};
EOF

cat > src/db/payments.ts <<'EOF'
import type pg from "pg";

import type { Charge, PaymentStatus } from "../payments/provider.js";
import type { Db } from "./client.js";

export interface Payment {
  id: string;
  invoiceId: string;
  chargeId: string;
  status: PaymentStatus;
  failureReason: string | null;
  createdAt: Date;
}

function fromRow(r: Record<string, any>): Payment {
  return {
    id: r.id,
    invoiceId: r.invoice_id,
    chargeId: r.charge_id,
    status: r.status,
    failureReason: r.failure_reason,
    createdAt: r.created_at,
  };
}

export async function recordPayment(
  client: pg.PoolClient,
  invoiceId: string,
  charge: Charge,
): Promise<Payment> {
  const { rows } = await client.query(
    `INSERT INTO payments
       (invoice_id, charge_id, status, amount_minor, currency, failure_reason)
     VALUES ($1, $2, $3, $4, $5, $6)
     RETURNING id, created_at`,
    [
      invoiceId,
      charge.id,
      charge.status,
      charge.amount.minor.toString(),
      charge.amount.currency,
      charge.failureReason ?? null,
    ],
  );
  return {
    id: rows[0].id,
    invoiceId,
    chargeId: charge.id,
    status: charge.status,
    failureReason: charge.failureReason ?? null,
    createdAt: rows[0].created_at,
  };
}

export async function findPayment(
  db: Db,
  id: string,
): Promise<Payment | null> {
  const { rows } = await db.query("SELECT * FROM payments WHERE id = $1", [id]);
  return rows[0] ? fromRow(rows[0]) : null;
}

export async function paymentsFor(
  db: Db,
  invoiceId: string,
): Promise<Payment[]> {
  const { rows } = await db.query(
    "SELECT * FROM payments WHERE invoice_id = $1 ORDER BY created_at DESC",
    [invoiceId],
  );
  return rows.map(fromRow);
}

/** Applies a status the provider reported after the fact. */
export async function setPaymentStatus(
  db: Db,
  chargeId: string,
  status: PaymentStatus,
): Promise<void> {
  await db.query("UPDATE payments SET status = $2 WHERE charge_id = $1", [
    chargeId,
    status,
  ]);
}
EOF

cat > src/server.ts <<'EOF'
import Fastify from "fastify";

import { loadConfig } from "./config.js";
import { createPool } from "./db/client.js";
import { logger } from "./lib/logger.js";
import { FakeProvider } from "./payments/fake.js";
import { healthRoutes } from "./routes/health.js";
import { invoiceRoutes } from "./routes/invoices.js";
import { paymentRoutes } from "./routes/payments.js";
import { refundRoutes } from "./routes/refunds.js";
import { webhookRoutes } from "./routes/webhooks.js";

const config = loadConfig();
const db = createPool(config.DATABASE_URL);
const app = Fastify({ logger: false });

if (config.PAYMENTS_PROVIDER !== "fake") {
  throw new Error("only the fake payments provider is wired up so far");
}
const provider = new FakeProvider();

app.register(healthRoutes, { db });
app.register(invoiceRoutes, { db, prefix: "/invoices" });
app.register(paymentRoutes, { db, provider, prefix: "/invoices" });
app.register(refundRoutes, { db, provider, prefix: "/payments" });
app.register(webhookRoutes, {
  db,
  secret: config.WEBHOOK_SECRET,
  prefix: "/webhooks",
});

app.listen({ port: config.PORT, host: "0.0.0.0" }).then(
  (address) => logger.info("listening", { address }),
  (err) => {
    logger.error("failed to start", { err: String(err) });
    process.exit(1);
  },
);
EOF
commit 71 "Add refunds"

# === 6 ======================================================================

cat > src/lib/retry.ts <<'EOF'
export interface RetryOptions {
  /** Total attempts, the first one included. */
  attempts: number;
  /** The delay before the first retry; it doubles each time. */
  baseDelayMs?: number;
  maxDelayMs?: number;
  /** Whether an error is worth another attempt. */
  retryIf?: (err: unknown) => boolean;
  onRetry?: (err: unknown, attempt: number, delayMs: number) => void;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Runs `fn`, retrying failures with exponential backoff and full jitter. */
export async function withRetry<T>(
  fn: () => Promise<T>,
  opts: RetryOptions,
): Promise<T> {
  const { attempts, onRetry, retryIf = () => true } = opts;
  const { baseDelayMs = 200, maxDelayMs = 5_000 } = opts;
  for (let attempt = 1; ; attempt++) {
    try {
      return await fn();
    } catch (err) {
      if (attempt >= attempts || !retryIf(err)) throw err;
      const cap = Math.min(maxDelayMs, baseDelayMs * 2 ** (attempt - 1));
      const delay = Math.round(Math.random() * cap);
      onRetry?.(err, attempt, delay);
      await sleep(delay);
    }
  }
}
EOF

cat > src/payments/provider.ts <<'EOF'
import type { Money } from "../lib/money.js";

export type PaymentStatus = "pending" | "succeeded" | "failed" | "refunded";

export interface Charge {
  id: string;
  status: PaymentStatus;
  amount: Money;
  failureReason?: string;
}

export interface ChargeRequest {
  amount: Money;
  /** A saved card or bank account. */
  methodId: string;
  /** Charging twice with the same key returns the first charge. */
  idempotencyKey: string;
}

export interface PaymentProvider {
  charge(req: ChargeRequest): Promise<Charge>;
  refund(chargeId: string, amount?: Money): Promise<Charge>;
}

/**
 * A failure talking to the provider. Retryable ones (timeouts, rate limits,
 * 5xx) are safe to repeat with the same idempotency key. A declined card is
 * not an error: it is a Charge with status "failed".
 */
export class ProviderError extends Error {
  constructor(
    message: string,
    readonly retryable: boolean,
  ) {
    super(message);
  }
}

export function isRetryable(err: unknown): boolean {
  return err instanceof ProviderError && err.retryable;
}
EOF

cat > src/payments/fake.ts <<'EOF'
import { randomUUID } from "node:crypto";

import {
  ProviderError,
  type Charge,
  type ChargeRequest,
  type PaymentProvider,
} from "./provider.js";

/** An in-memory provider for development and tests. */
export class FakeProvider implements PaymentProvider {
  readonly charges = new Map<string, Charge>();
  private byKey = new Map<string, string>();
  private outages = 0;

  /** Makes the next `n` calls fail as if the provider timed out. */
  failNext(n: number) {
    this.outages = n;
  }

  async charge(req: ChargeRequest): Promise<Charge> {
    if (this.outages > 0) {
      this.outages--;
      throw new ProviderError("provider timed out", true);
    }
    const seen = this.byKey.get(req.idempotencyKey);
    if (seen) return this.charges.get(seen)!;

    const declined = req.methodId.startsWith("pm_decline");
    const charge: Charge = {
      id: `ch_${randomUUID()}`,
      status: declined ? "failed" : "succeeded",
      amount: req.amount,
      failureReason: declined ? "card_declined" : undefined,
    };
    this.charges.set(charge.id, charge);
    this.byKey.set(req.idempotencyKey, charge.id);
    return charge;
  }

  async refund(chargeId: string): Promise<Charge> {
    const charge = this.charges.get(chargeId);
    if (!charge) throw new Error(`no such charge: ${chargeId}`);
    return { ...charge, status: "refunded" };
  }
}
EOF

cat > src/routes/payments.ts <<'EOF'
import type { FastifyPluginAsync } from "fastify";
import { z } from "zod";

import { inTransaction, type Db } from "../db/client.js";
import { findInvoice, markPaid } from "../db/invoices.js";
import { recordPayment } from "../db/payments.js";
import { Conflict, NotFound, PaymentFailed } from "../lib/errors.js";
import { logger } from "../lib/logger.js";
import { withRetry } from "../lib/retry.js";
import { isRetryable, type PaymentProvider } from "../payments/provider.js";

const Params = z.object({ id: z.string().uuid() });
const Body = z.object({ methodId: z.string().min(1) });

interface Options {
  db: Db;
  provider: PaymentProvider;
  /** Extra attempts when the provider has a transient failure. */
  maxRetries: number;
}

export const paymentRoutes: FastifyPluginAsync<Options> = async (
  app,
  { db, provider, maxRetries },
) => {
  app.post("/:id/pay", async (req, reply) => {
    const { id } = Params.parse(req.params);
    const { methodId } = Body.parse(req.body);

    const invoice = await findInvoice(db, id);
    if (!invoice) throw new NotFound(`invoice ${id}`);
    if (invoice.status !== "open") {
      throw new Conflict(`invoice ${id} is ${invoice.status}`);
    }

    // The same key on every attempt: a retry after a timeout must not
    // charge the customer twice.
    const request = {
      amount: invoice.total,
      methodId,
      idempotencyKey: `invoice-${id}`,
    };
    const charge = await withRetry(() => provider.charge(request), {
      attempts: maxRetries + 1,
      retryIf: isRetryable,
      onRetry: (err, attempt, delayMs) =>
        logger.warn("retrying charge", {
          invoice: id,
          attempt,
          delayMs,
          err: String(err),
        }),
    });

    const payment = await inTransaction(db, async (client) => {
      const payment = await recordPayment(client, id, charge);
      if (charge.status === "succeeded") await markPaid(client, id);
      return payment;
    });

    if (charge.status === "failed") {
      logger.warn("charge failed", { invoice: id, reason: charge.failureReason });
      throw new PaymentFailed(charge.failureReason ?? "unknown");
    }
    logger.info("invoice paid", { invoice: id, charge: charge.id });
    return reply.code(201).send(payment);
  });
};
EOF

cat > src/config.ts <<'EOF'
import { z } from "zod";

const Env = z
  .object({
    PORT: z.coerce.number().default(3000),
    DATABASE_URL: z.string().url(),
    LOG_LEVEL: z.enum(["debug", "info", "warn", "error"]).default("info"),
    PAYMENTS_PROVIDER: z.enum(["fake", "live"]).default("fake"),
    PAYMENTS_API_KEY: z.string().optional(),
    PAYMENTS_MAX_RETRIES: z.coerce.number().int().min(0).max(10).default(3),
    WEBHOOK_SECRET: z.string().min(16),
  })
  .refine((env) => env.PAYMENTS_PROVIDER === "fake" || env.PAYMENTS_API_KEY, {
    message: "required when PAYMENTS_PROVIDER=live",
    path: ["PAYMENTS_API_KEY"],
  });

export type Config = z.infer<typeof Env>;

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const parsed = Env.safeParse(env);
  if (!parsed.success) {
    const issues = parsed.error.issues.map(
      (i) => `${i.path.join(".")}: ${i.message}`,
    );
    throw new Error(`invalid configuration:\n  ${issues.join("\n  ")}`);
  }
  return parsed.data;
}
EOF

cat > src/server.ts <<'EOF'
import Fastify from "fastify";

import { loadConfig } from "./config.js";
import { createPool } from "./db/client.js";
import { logger } from "./lib/logger.js";
import { FakeProvider } from "./payments/fake.js";
import { healthRoutes } from "./routes/health.js";
import { invoiceRoutes } from "./routes/invoices.js";
import { paymentRoutes } from "./routes/payments.js";
import { refundRoutes } from "./routes/refunds.js";
import { webhookRoutes } from "./routes/webhooks.js";

const config = loadConfig();
const db = createPool(config.DATABASE_URL);
const app = Fastify({ logger: false });

if (config.PAYMENTS_PROVIDER !== "fake") {
  throw new Error("only the fake payments provider is wired up so far");
}
const provider = new FakeProvider();

app.register(healthRoutes, { db });
app.register(invoiceRoutes, { db, prefix: "/invoices" });
app.register(paymentRoutes, {
  db,
  provider,
  maxRetries: config.PAYMENTS_MAX_RETRIES,
  prefix: "/invoices",
});
app.register(refundRoutes, { db, provider, prefix: "/payments" });
app.register(webhookRoutes, {
  db,
  secret: config.WEBHOOK_SECRET,
  prefix: "/webhooks",
});

app.listen({ port: config.PORT, host: "0.0.0.0" }).then(
  (address) => logger.info("listening", { address }),
  (err) => {
    logger.error("failed to start", { err: String(err) });
    process.exit(1);
  },
);
EOF
commit 55 "Retry transient charge failures with backoff"

# === 7 ======================================================================

cat > src/lib/logger.ts <<'EOF'
import { AsyncLocalStorage } from "node:async_hooks";

type Level = "debug" | "info" | "warn" | "error";
type Fields = Record<string, unknown>;

const order: Record<Level, number> = {
  debug: 10,
  info: 20,
  warn: 30,
  error: 40,
};
const threshold = (process.env.LOG_LEVEL as Level | undefined) ?? "info";

/** Fields added to every line logged while handling a request. */
const context = new AsyncLocalStorage<Fields>();

export function withLogContext<T>(fields: Fields, fn: () => T): T {
  return context.run({ ...context.getStore(), ...fields }, fn);
}

function write(level: Level, msg: string, fields: Fields = {}) {
  if (order[level] < order[threshold]) return;
  const line = {
    time: new Date().toISOString(),
    level,
    msg,
    ...context.getStore(),
    ...fields,
  };
  process.stdout.write(JSON.stringify(line) + "\n");
}

export const logger = {
  debug: (msg: string, fields?: Fields) => write("debug", msg, fields),
  info: (msg: string, fields?: Fields) => write("info", msg, fields),
  warn: (msg: string, fields?: Fields) => write("warn", msg, fields),
  error: (msg: string, fields?: Fields) => write("error", msg, fields),
};
EOF

cat > src/server.ts <<'EOF'
import { randomUUID } from "node:crypto";

import Fastify from "fastify";

import { loadConfig } from "./config.js";
import { createPool } from "./db/client.js";
import { logger, withLogContext } from "./lib/logger.js";
import { FakeProvider } from "./payments/fake.js";
import { healthRoutes } from "./routes/health.js";
import { invoiceRoutes } from "./routes/invoices.js";
import { paymentRoutes } from "./routes/payments.js";
import { refundRoutes } from "./routes/refunds.js";
import { webhookRoutes } from "./routes/webhooks.js";

const config = loadConfig();
const db = createPool(config.DATABASE_URL);
const app = Fastify({
  logger: false,
  genReqId: (req) => String(req.headers["x-request-id"] ?? randomUUID()),
});

if (config.PAYMENTS_PROVIDER !== "fake") {
  throw new Error("only the fake payments provider is wired up so far");
}
const provider = new FakeProvider();

// Every line logged while handling a request carries its id.
app.addHook("onRequest", (req, _reply, done) => {
  withLogContext({ requestId: req.id }, done);
});

app.register(healthRoutes, { db });
app.register(invoiceRoutes, { db, prefix: "/invoices" });
app.register(paymentRoutes, {
  db,
  provider,
  maxRetries: config.PAYMENTS_MAX_RETRIES,
  prefix: "/invoices",
});
app.register(refundRoutes, { db, provider, prefix: "/payments" });
app.register(webhookRoutes, {
  db,
  secret: config.WEBHOOK_SECRET,
  prefix: "/webhooks",
});

app.listen({ port: config.PORT, host: "0.0.0.0" }).then(
  (address) => logger.info("listening", { address }),
  (err) => {
    logger.error("failed to start", { err: String(err) });
    process.exit(1);
  },
);
EOF
commit 41 "Log a request id with every line"

# === 8 ======================================================================

cat > src/payments/live.ts <<'EOF'
import type { Money } from "../lib/money.js";
import {
  ProviderError,
  type Charge,
  type ChargeRequest,
  type PaymentProvider,
} from "./provider.js";

const BASE_URL = "https://api.payments.example/v1";
const TIMEOUT_MS = 10_000;

/** The real provider, over its REST API. */
export class LiveProvider implements PaymentProvider {
  constructor(private readonly apiKey: string) {}

  charge(req: ChargeRequest): Promise<Charge> {
    const body = {
      amount: req.amount.minor.toString(),
      currency: req.amount.currency.toLowerCase(),
      payment_method: req.methodId,
    };
    return this.post("/charges", body, req.idempotencyKey);
  }

  refund(chargeId: string, amount?: Money): Promise<Charge> {
    const body = { amount: amount?.minor.toString() };
    return this.post(`/charges/${chargeId}/refunds`, body);
  }

  private async post(
    path: string,
    body: object,
    idempotencyKey?: string,
  ): Promise<Charge> {
    const headers: Record<string, string> = {
      authorization: `Bearer ${this.apiKey}`,
      "content-type": "application/json",
    };
    if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;

    let res: Response;
    try {
      res = await fetch(BASE_URL + path, {
        method: "POST",
        headers,
        body: JSON.stringify(body),
        signal: AbortSignal.timeout(TIMEOUT_MS),
      });
    } catch (err) {
      // A timeout or a dropped connection: the charge may or may not have
      // happened, so retry with the same idempotency key.
      throw new ProviderError(`request failed: ${err}`, true);
    }
    // Rate limits and server errors are worth another attempt.
    const retryable = res.status === 429 || res.status >= 500;
    if (!res.ok) {
      throw new ProviderError(`provider returned ${res.status}`, retryable);
    }
    return toCharge(await res.json());
  }
}

interface RawCharge {
  id: string;
  status: Charge["status"];
  amount: string;
  currency: string;
  failure_code?: string;
}

function toCharge(raw: RawCharge): Charge {
  return {
    id: raw.id,
    status: raw.status,
    amount: { minor: BigInt(raw.amount), currency: raw.currency.toUpperCase() },
    failureReason: raw.failure_code,
  };
}
EOF

cat > .env.example <<'EOF'
DATABASE_URL=postgres://localhost:5432/billing
PAYMENTS_PROVIDER=fake
PAYMENTS_API_KEY=
WEBHOOK_SECRET=change-me-to-something-long
EOF

perl -0pi -e '
  s|(import \{ FakeProvider \} from "./payments/fake.js";\n)|$1import { LiveProvider } from "./payments/live.js";\n|;
  s|if \(config.PAYMENTS_PROVIDER !== "fake"\) \{\n.*?\n\}\nconst provider = new FakeProvider\(\);|const provider =\n  config.PAYMENTS_PROVIDER === "live"\n    ? new LiveProvider(config.PAYMENTS_API_KEY!)\n    : new FakeProvider();|s;
' src/server.ts
commit 30 "Wire up the live payment provider"

# === 9 ======================================================================

cat > src/lib/money.ts <<'EOF'
export interface Money {
  /** The amount in the currency's minor unit, such as cents. */
  minor: bigint;
  currency: string;
}

/** Currencies whose minor unit is not a hundredth (ISO 4217). */
const EXPONENTS: Record<string, number> = {
  JPY: 0,
  KRW: 0,
  VND: 0,
  BHD: 3,
  KWD: 3,
};

/** Digits after the decimal point: 2 for USD, 0 for JPY. */
export function exponent(currency: string): number {
  return EXPONENTS[currency] ?? 2;
}

export function add(a: Money, b: Money): Money {
  if (a.currency !== b.currency) {
    throw new Error(`currency mismatch: ${a.currency} vs ${b.currency}`);
  }
  return { minor: a.minor + b.minor, currency: a.currency };
}

export function format(m: Money): string {
  const digits = exponent(m.currency);
  const major = Number(m.minor) / 10 ** digits;
  const fmt = new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: m.currency,
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
  return fmt.format(major);
}
EOF

cat > test/money.test.ts <<'EOF'
import { describe, expect, it } from "vitest";

import { add, exponent, format } from "../src/lib/money.js";

describe("money", () => {
  it("adds amounts in the same currency", () => {
    const a = { minor: 150n, currency: "USD" };
    const b = { minor: 250n, currency: "USD" };
    expect(add(a, b)).toEqual({ minor: 400n, currency: "USD" });
  });

  it("refuses to mix currencies", () => {
    const usd = { minor: 1n, currency: "USD" };
    const eur = { minor: 1n, currency: "EUR" };
    expect(() => add(usd, eur)).toThrow(/mismatch/);
  });

  it("formats dollars", () => {
    expect(format({ minor: 123456n, currency: "USD" })).toBe("$1,234.56");
  });

  it("formats yen without dividing by 100", () => {
    expect(exponent("JPY")).toBe(0);
    expect(format({ minor: 1500n, currency: "JPY" })).toBe("¥1,500");
  });

  it("formats three-decimal currencies", () => {
    expect(format({ minor: 1234n, currency: "BHD" })).toBe("BHD 1.234");
  });
});
EOF
commit 19 "Fix formatting of zero-decimal currencies"

# === 10 =====================================================================

cat > test/webhooks.test.ts <<'EOF'
import { createHmac } from "node:crypto";
import { describe, expect, it } from "vitest";

import { SignatureError, verifyWebhook } from "../src/payments/webhooks.js";

const secret = "whsec_test_0123456789abcdef";
const now = 1_700_000_000;
const body = JSON.stringify({
  id: "evt_1",
  type: "charge.succeeded",
  chargeId: "ch_1",
  createdAt: now,
});

function sign(payload: string, t: number, key = secret) {
  const v1 = createHmac("sha256", key).update(`${t}.${payload}`).digest("hex");
  return `t=${t},v1=${v1}`;
}

describe("verifyWebhook", () => {
  it("accepts a fresh, correctly signed event", () => {
    const event = verifyWebhook(body, sign(body, now), secret, now);
    expect(event.chargeId).toBe("ch_1");
  });

  it("rejects a tampered body", () => {
    const header = sign(body, now);
    const tampered = body.replace("ch_1", "ch_2");
    expect(() => verifyWebhook(tampered, header, secret, now)).toThrow(
      SignatureError,
    );
  });

  it("rejects stale timestamps", () => {
    const header = sign(body, now - 600);
    expect(() => verifyWebhook(body, header, secret, now)).toThrow(/tolerance/);
  });

  it("rejects the wrong secret", () => {
    const header = sign(body, now, "whsec_some_other_secret");
    expect(() => verifyWebhook(body, header, secret, now)).toThrow(/mismatch/);
  });

  it("requires the header", () => {
    expect(() => verifyWebhook(body, undefined, secret, now)).toThrow(/missing/);
  });
});
EOF

cat > test/retry.test.ts <<'EOF'
import { describe, expect, it, vi } from "vitest";

import { withRetry } from "../src/lib/retry.js";

describe("withRetry", () => {
  it("returns the first success", async () => {
    const fn = vi
      .fn()
      .mockRejectedValueOnce(new Error("flaky"))
      .mockResolvedValue("ok");
    const opts = { attempts: 3, baseDelayMs: 0 };
    await expect(withRetry(fn, opts)).resolves.toBe("ok");
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it("gives up after the last attempt", async () => {
    const fn = vi.fn().mockRejectedValue(new Error("down"));
    const opts = { attempts: 3, baseDelayMs: 0 };
    await expect(withRetry(fn, opts)).rejects.toThrow("down");
    expect(fn).toHaveBeenCalledTimes(3);
  });

  it("stops at errors that are not worth retrying", async () => {
    const fn = vi.fn().mockRejectedValue(new Error("card declined"));
    const opts = { attempts: 5, baseDelayMs: 0, retryIf: () => false };
    await expect(withRetry(fn, opts)).rejects.toThrow("declined");
    expect(fn).toHaveBeenCalledTimes(1);
  });
});
EOF
commit 11 "Add webhook and retry tests"

# === 11 =====================================================================

cat > README.md <<'EOF'
# billing-api

Invoices for the shop, served over HTTP, and payments for them.

    cp .env.example .env
    npm install
    npm run dev

| Route | |
|---|---|
| `GET /invoices?customer=<id>` | A customer's invoices |
| `GET /invoices/<id>` | One invoice |
| `POST /invoices/<id>/pay` | Charge a saved payment method |
| `POST /payments/<id>/refund` | Refund a payment in full |
| `POST /webhooks/payments` | Status updates from the provider |

## Payments

Charges go through a `PaymentProvider`. In development the fake one keeps
everything in memory: a method id starting with `pm_decline` is declined,
and `failNext(n)` simulates an outage. Transient failures are retried with
backoff, always with the same idempotency key, so a retry never charges a
customer twice.

Webhooks are signed: each request carries `x-signature: t=<unix>,v1=<hex>`,
an HMAC-SHA256 of `<t>.<body>` with `WEBHOOK_SECRET`. Requests older than
five minutes are rejected.
EOF

cat > .env.example <<'EOF'
# Copy to .env and fill in.
DATABASE_URL=postgres://localhost:5432/billing
LOG_LEVEL=info

# "fake" keeps everything in memory; "live" needs an API key.
PAYMENTS_PROVIDER=fake
PAYMENTS_API_KEY=
# Attempts after a transient failure (timeouts, 429, 5xx).
PAYMENTS_MAX_RETRIES=3

# Shared with the provider; at least 16 characters.
WEBHOOK_SECRET=change-me-to-something-long
EOF
commit 4 "Document the payments API"

# === Uncommitted: the agent is making POST /pay idempotent ===================

cat > migrations/004_idempotency.sql <<'EOF'
CREATE TABLE idempotency_keys (
  key text PRIMARY KEY,
  status smallint NOT NULL,
  body jsonb NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now()
);
EOF

cat > src/lib/idempotency.ts <<'EOF'
import type { Db } from "../db/client.js";

export interface StoredResponse {
  status: number;
  body: unknown;
}

/** The response sent earlier for the same Idempotency-Key, if any. */
export async function replay(
  db: Db,
  key: string,
): Promise<StoredResponse | null> {
  const { rows } = await db.query(
    `SELECT status, body FROM idempotency_keys
     WHERE key = $1 AND created_at > now() - interval '24 hours'`,
    [key],
  );
  return rows[0] ?? null;
}

export async function remember(
  db: Db,
  key: string,
  res: StoredResponse,
): Promise<void> {
  await db.query(
    `INSERT INTO idempotency_keys (key, status, body) VALUES ($1, $2, $3)
     ON CONFLICT DO NOTHING`,
    [key, res.status, JSON.stringify(res.body)],
  );
}
EOF

perl -0pi -e '
  s|(import \{ logger \})|import { remember, replay } from "../lib/idempotency.js";\n$1|;
  s|(    const \{ methodId \} = Body.parse\(req.body\);\n)|$1\n    const key = req.headers["idempotency-key"];\n    if (typeof key === "string") {\n      const earlier = await replay(db, key);\n      if (earlier) return reply.code(earlier.status).send(earlier.body);\n    }\n|;
  s|(    return reply.code\(201\).send\(payment\);)|    if (typeof key === "string") {\n      await remember(db, key, { status: 201, body: payment });\n    }\n$1|;
' src/routes/payments.ts

perl -pi -e 's/\| Charge a saved payment method \|/| Charge a saved payment method; a repeated `Idempotency-Key` gets the first response |/' README.md

# === Viewed marks: commits 1 to 4, and part of 5 ===========================

mkdir -p .git/spotter
{
  printf '{"version":1,"viewed":{'
  sep=
  for spec in HEAD~10 HEAD~9 HEAD~8 HEAD~7 \
    HEAD~6:migrations/003_refunds.sql HEAD~6:src/payments/provider.ts; do
    rev=${spec%%:*}
    only=
    [ "$rev" != "$spec" ] && only=${spec#*:}
    while read -r _ _ old new _ path; do
      if [ -n "$only" ] && [ "$path" != "$only" ]; then continue; fi
      printf '%s"%s:%s:%s":%s' "$sep" "$old" "$new" "$path" "$NOW"
      sep=,
    done < <(git diff-tree -r --no-abbrev --no-commit-id "$rev")
  done
  printf '}}\n'
} > .git/spotter/viewed.json
