import assert from "node:assert/strict";
import test from "node:test";
import { ulid } from "ulid";
import { ACCESS_TTL_SEC, bearerToken, nowSeconds, randomSecret, signToken, validLoopbackRedirect, verifyToken } from "../src/auth.ts";
import type { Env } from "../src/env.ts";

const env = { AUTH_SIGNING_KEY: randomSecret(), PUBLIC_ORIGIN: "https://relay.example" } as Env;
test("only explicit loopback OAuth callbacks are accepted", () => {
  assert.equal(validLoopbackRedirect("http://127.0.0.1:43025/oauth/callback"), true);
  for (const url of ["http://127.0.0.1/oauth/callback", "https://127.0.0.1:43025/oauth/callback", "http://evil.example/oauth/callback", "http://127.0.0.1:43025/oauth/callback?next=https://x", "http://user@127.0.0.1:1234/oauth/callback"]) assert.equal(validLoopbackRedirect(url), false);
});
test("JWTs separate access/refresh, audience, signature, expiry and malformed inputs", async () => {
  const claims = { sub: "google-sub", email: "user@example.test", sid: ulid() };
  const token = await signToken(env, "access", claims, nowSeconds() + ACCESS_TTL_SEC);
  assert.equal((await verifyToken(env, token, "access"))?.sub, claims.sub);
  assert.equal(await verifyToken(env, token, "refresh"), null);
  assert.equal(await verifyToken({ ...env, PUBLIC_ORIGIN: "https://another.example" }, token, "access"), null);
  assert.equal(await verifyToken({ ...env, AUTH_SIGNING_KEY: randomSecret() }, token, "access"), null);
  assert.equal(await verifyToken(env, await signToken(env, "access", claims, nowSeconds()), "access"), null);
  assert.equal(await verifyToken(env, await signToken(env, "access", claims, nowSeconds() + 3600), "access"), null);
  for (const invalid of ["", "a.b.c", "....", "%.%.$", token.slice(0, -6) + "oops"]) {
    assert.equal(await verifyToken(env, invalid, "access"), null);
  }
});
test("credentials are accepted only in a bounded bearer header", () => {
  assert.equal(
    bearerToken(
      new Request("https://relay.example/relay", {
        headers: { authorization: "Bearer abc.def.ghi" },
      }),
    ),
    "abc.def.ghi",
  );
  assert.equal(bearerToken(new Request("https://relay.example/relay?token=secret")), null);
  assert.equal(bearerToken(new Request("https://relay.example/relay", { headers: { authorization: "Basic secret" } })), null);
});
