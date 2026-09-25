import assert from "node:assert/strict";
import test from "node:test";
import { ulid } from "ulid";
import { randomSecret, type Tokens } from "../src/auth.ts";
import { harness } from "./harness.ts";
import { connect, identity, nothingPending } from "./relay-client.ts";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

function auth(token: string) {
  return { authorization: "Bearer " + token, "content-type": "application/json" };
}

test("only an operator can restart the relay", async () => {
  const h = await harness();
  try {
    const client = await connect(h);
    const session = await h.login();
    for (const token of ["invalid", session.access_token]) {
      assert.equal((await h.fetch("/v1/admin/relay/restart", { method: "POST", headers: auth(token) })).status, 401);
    }
    assert.ok(await nothingPending(client), "refused restarts leave relay connections alone");
    assert.equal((await h.fetch("/v1/admin/relay/restart", { method: "POST", headers: auth(h.adminToken) })).status, 200);
    assert.equal(await client.closed, 1012);
  } finally {
    await h.close();
  }
});

test("logout does not end a native relay connection", { timeout: 10000 }, async () => {
  const h = await harness();
  try {
    const session = await h.login();
    const client = await connect(h, identity(), { headers: auth(session.access_token) });
    assert.equal(
      (
        await h.fetch("/v1/auth/logout", {
          method: "POST",
          headers: auth(session.refresh_token),
          body: '{"all":false}',
        })
      ).status,
      200,
    );
    assert.ok(await nothingPending(client));
    client.close();
  } finally {
    await h.close();
  }
});

test("empty-frame flooding is limited even when it consumes no payload bytes", { timeout: 10000 }, async () => {
  const h = await harness();
  try {
    const session = await h.login();
    const upgrade = (token: string) =>
      h.fetch("/relay", {
        headers: {
          ...auth(token),
          upgrade: "websocket",
          "sec-websocket-protocol": "iroh-relay-v2",
        },
      });
    const response = await upgrade(session.access_token);
    assert.equal(response.status, 101);
    const socket = response.webSocket!;
    socket.accept();
    const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_HUB");
    await budgets.get(budgets.idFromName("primary")).exhaustBudget("frames");
    const closed = new Promise<number>((resolve) => socket.addEventListener("close", (event) => resolve(event.code), { once: true }));
    socket.send(new Uint8Array(0));
    assert.equal(await closed, 4008);
    const anotherSession = await h.login();
    assert.equal((await upgrade(anotherSession.access_token)).status, 429);
  } finally {
    await h.close();
  }
});

test("session rotations and revocations survive a Worker restart", { timeout: 20000 }, async () => {
  const persist = await fs.mkdtemp(path.join(os.tmpdir(), "zork-account-do-"));
  const signingKey = randomSecret();
  let h = await harness({ persist, signingKey });
  try {
    const session = await h.login();
    await h.close();
    h = await harness({ persist, signingKey });
    const rotated = await h.fetch("/v1/auth/refresh", {
      method: "POST",
      headers: auth(session.refresh_token),
      body: JSON.stringify({ request_id: ulid() }),
    });
    assert.equal(rotated.status, 200);
    const next = (await rotated.json()) as Tokens;
    assert.equal(
      (
        await h.fetch("/v1/auth/logout", {
          method: "POST",
          headers: auth(next.refresh_token),
          body: '{"all":true}',
        })
      ).status,
      200,
    );
    await h.close();
    h = await harness({ persist, signingKey });
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(next.access_token) })).status, 401);
    assert.equal(
      (
        await h.fetch("/v1/auth/refresh", {
          method: "POST",
          headers: auth(next.refresh_token),
          body: JSON.stringify({ request_id: ulid() }),
        })
      ).status,
      401,
    );
  } finally {
    await h.close();
    await fs.rm(persist, { recursive: true, force: true });
  }
});

test("real Worker login, rotation retries/reuse, session revocation and account isolation", { timeout: 20000 }, async () => {
  const h = await harness();
  try {
    const flow = await h.begin();
    const oauthCallback =
      "/v1/auth/google/callback?" +
      new URLSearchParams({
        state: flow.google.searchParams.get("state")!,
        code: flow.googleCode,
      });
    assert.equal((await h.fetch(oauthCallback, { redirect: "manual" })).status, 400, "callback needs browser binding");
    const completed = await h.complete(flow);
    assert.equal(completed.redirect.searchParams.get("state"), flow.state);
    assert.equal(completed.redirect.searchParams.has("token"), false);
    assert.equal((await h.exchange(flow, completed.code, randomSecret())).status, 401);
    const exchange = await h.exchange(flow, completed.code);
    assert.equal(exchange.status, 200);
    const original = (await exchange.json()) as Tokens;
    assert.equal((await h.exchange(flow, completed.code)).status, 401, "code is single use");
    assert.equal((await h.fetch(oauthCallback, { headers: { cookie: flow.cookie }, redirect: "manual" })).status, 400);
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(original.refresh_token) })).status, 401, "refresh is not access");
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(original.access_token) })).status, 200);
    const other = await h.login("different-account");
    const requestId = ulid();
    const refresh = () =>
      h.fetch("/v1/auth/refresh", {
        method: "POST",
        headers: auth(original.refresh_token),
        body: JSON.stringify({ request_id: requestId }),
      });
    const first = await refresh(),
      retry = await refresh();
    assert.equal(first.status, 200);
    const next = (await first.json()) as Tokens;
    assert.equal(JSON.stringify(await retry.json()) === JSON.stringify(next), true, "lost response retry returns the identical rotation");
    assert.equal(next.refresh_token !== original.refresh_token, true);
    const reused = await h.fetch("/v1/auth/refresh", {
      method: "POST",
      headers: auth(original.refresh_token),
      body: JSON.stringify({ request_id: ulid() }),
    });
    assert.equal(reused.status, 401);
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(next.access_token) })).status, 401, "reuse revokes access");
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(other.access_token) })).status, 200, "other account remains valid");
    const loggedOut = await h.fetch("/v1/auth/logout", {
      method: "POST",
      headers: auth(other.refresh_token),
      body: '{"all":false}',
    });
    assert.equal(loggedOut.status, 200);
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(other.access_token) })).status, 401);
    assert.equal(
      (
        await h.fetch("/v1/auth/logout", {
          method: "POST",
          headers: auth(other.refresh_token),
          body: '{"all":false}',
        })
      ).status,
      200,
    );
  } finally {
    await h.close();
  }
});

test("Google identity validation rejects wrong nonce, audience and unverified email", { timeout: 20000 }, async () => {
  const h = await harness();
  try {
    for (const invalid of ["nonce", "aud", "email"]) {
      const flow = await h.begin("google-test-user", invalid);
      const callback = await h.complete(flow);
      assert.equal(callback.redirect.searchParams.get("error"), "google_login_failed");
      assert.equal(callback.redirect.searchParams.has("code"), false);
    }
  } finally {
    await h.close();
  }
});

test("cloud logout, expired credentials and account blocking do not govern Mesh transport", { timeout: 15000 }, async () => {
  const h = await harness();
  try {
    const session = await h.login();
    const client = await connect(h, identity(), { path: "/relay?token=ignored", headers: { ...auth(session.access_token), cookie: "private=ignored" } });
    assert.equal((await h.fetch("/v1/auth/logout", { method: "POST", headers: auth(session.refresh_token), body: '{"all":true}' })).status, 200);
    assert.equal((await h.fetch("/v1/auth/session", { headers: auth(session.access_token) })).status, 401);
    assert.ok(await nothingPending(client));
    assert.equal((await h.fetch("/v1/admin/accounts/" + session.subject, { method: "POST", headers: auth(h.adminToken), body: '{"blocked":true}' })).status, 200);
    assert.ok(await nothingPending(client));
    const expired = await h.fetch("/__test/access", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ sub: session.subject, sid: session.session_id, seconds: -1 }) });
    const reconnect = await connect(h, identity(), { headers: auth(((await expired.json()) as any).token) });
    assert.ok(await nothingPending(reconnect));
    reconnect.close();
    client.close();
  } finally {
    await h.close();
  }
});

test("relay budgets span anonymous and account sessions; account idle expiry still prevents renewal", { timeout: 20000 }, async () => {
  const h = await harness();
  const sockets: Array<{ close(): void }> = [];
  try {
    const first = await h.login(),
      second = await h.login();
    const upgrade = (token: string) =>
      h.fetch("/relay", {
        headers: { ...auth(token), upgrade: "websocket", "sec-websocket-protocol": "iroh-relay-v2" },
      });
    // All local test clients share one IP budget.
    for (let i = 0; i < 32; i++) {
      const response = await upgrade((i % 2 ? first : second).access_token);
      assert.equal(response.status, 101);
      response.webSocket!.accept();
      sockets.push(response.webSocket!);
    }
    assert.equal((await upgrade(second.access_token)).status, 429);
    const accounts: any = await h.mf.getDurableObjectNamespace("ACCOUNTS");
    const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_HUB");
    await budgets.get(budgets.idFromName("primary")).exhaustBudget();
    const close = new Promise<number>((resolve) =>
      (sockets[0] as any).addEventListener("close", (event: any) => resolve(event.code), {
        once: true,
      }),
    );
    (sockets[0] as any).send(new Uint8Array([1]));
    assert.equal(await close, 4008);
    assert.equal((await upgrade(second.access_token)).status, 429, "reconnecting does not reset bytes");
    const other = await h.login("different-account");
    const response = await upgrade(other.access_token);
    assert.equal(response.status, 429, "a different account cannot bypass the relay budget");
    const isolated = accounts.get(accounts.idFromName(other.subject)) as any;
    await isolated.expire(other.session_id, "idle");
    assert.equal(
      (
        await h.fetch("/v1/auth/refresh", {
          method: "POST",
          headers: auth(other.refresh_token),
          body: JSON.stringify({ request_id: ulid() }),
        })
      ).status,
      401,
    );
  } finally {
    for (const socket of sockets) socket.close();
    await h.close();
  }
});
