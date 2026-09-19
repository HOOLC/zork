import assert from "node:assert/strict";
import test from "node:test";
import { randomSecret, digest, type Tokens } from "../src/auth.ts";
import { harness } from "./harness.ts";

const json = (body: object) => ({ method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
async function start(h: Awaited<ReturnType<typeof harness>>) {
  const id = randomSecret(),
    verifier = randomSecret();
  const response = await h.fetch("/v1/auth/device", json({ id, code_challenge: await digest(verifier), name: "Station <unsafe>" }));
  assert.equal(response.status, 200);
  const result = (await response.json()) as any;
  assert.equal(result.verification_uri, h.origin + "/v1/auth/device/" + id);
  return { id, verifier, path: "/v1/auth/device/" + id };
}
async function approve(h: Awaited<ReturnType<typeof harness>>, flow: Awaited<ReturnType<typeof start>>) {
  const page = await h.fetch(flow.path);
  const cookie = page.headers.get("set-cookie")!.split(";")[0];
  const html = await page.text();
  assert.match(html, /Station &lt;unsafe&gt;/);
  const csrf = /name="csrf" value="([^"]+)"/.exec(html)![1];
  const body = new URLSearchParams({ csrf }).toString();
  const post = await h.fetch(flow.path, { method: "POST", body, redirect: "manual", headers: { cookie, origin: h.origin, "content-type": "application/x-www-form-urlencoded" } });
  assert.equal(post.status, 302);
  const google = new URL(post.headers.get("location")!);
  const code = randomSecret();
  h.codes.set(code, { nonce: google.searchParams.get("nonce")!, challenge: google.searchParams.get("code_challenge")!, sub: "device-user" });
  const complete = await h.fetch("/v1/auth/google/callback?" + new URLSearchParams({ state: flow.id, code }), { redirect: "manual", headers: { cookie } });
  assert.equal(complete.headers.get("location"), h.origin + "/v1/auth/device/complete");
}

test("headless login requires browser confirmation, CSRF, PKCE; lost poll responses are replayable", { timeout: 20000 }, async () => {
  const h = await harness();
  try {
    const flow = await start(h);
    const poll = (verifier = flow.verifier) => h.fetch("/v1/auth/device/token", json({ id: flow.id, code_verifier: verifier }));
    assert.equal((await poll(randomSecret())).status, 401);
    assert.equal((await poll()).status, 202);
    assert.equal((await poll()).status, 429);
    assert.equal((await h.fetch(flow.path, { method: "POST", body: "csrf=wrong", headers: { origin: "https://attacker.test", "content-type": "application/x-www-form-urlencoded" } })).status, 400);
    await approve(h, flow);
    const response = await poll();
    assert.equal(response.status, 200);
    const tokens = (await response.json()) as Tokens;
    assert.deepEqual(await (await poll()).json(), tokens);
    assert.equal((await h.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + tokens.access_token } })).status, 200);
    assert.equal((await h.fetch("/v1/auth/device/cancel", json({ id: flow.id, code_verifier: flow.verifier }))).status, 200);
    assert.equal((await h.fetch("/v1/auth/session", { headers: { authorization: "Bearer " + tokens.access_token } })).status, 401);
    assert.equal((await poll()).status, 403);
  } finally {
    await h.close();
  }
});

test("cancelling an unclaimed device approval cannot issue a session later", { timeout: 20000 }, async () => {
  const h = await harness();
  try {
    const flow = await start(h);
    await approve(h, flow);
    assert.equal((await h.fetch("/v1/auth/device/cancel", json({ id: flow.id, code_verifier: randomSecret() }))).status, 401);
    assert.equal((await h.fetch("/v1/auth/device/cancel", json({ id: flow.id, code_verifier: flow.verifier }))).status, 200);
    assert.equal((await h.fetch("/v1/auth/device/token", json({ id: flow.id, code_verifier: flow.verifier }))).status, 403);
  } finally {
    await h.close();
  }
});
