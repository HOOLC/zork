import assert from "node:assert/strict";
import test from "node:test";
import { generateKeyPairSync, sign } from "node:crypto";
import { harness } from "./harness.ts";
import type { Tokens } from "../src/auth.ts";
function device(session: Tokens, station: boolean) {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const bytes = publicKey.export({ format: "der", type: "spki" }).subarray(-32);
  let value = "",
    acc = 0,
    bits = 0;
  for (const byte of bytes) {
    acc = (acc << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      value += "ybndrfg8ejkmcpqxot1uwisza345h769"[(acc >>> bits) & 31];
    }
  }
  if (bits) value += "ybndrfg8ejkmcpqxot1uwisza345h769"[(acc << (5 - bits)) & 31];
  const origin = `key:${value}`;
  return {
    origin,
    name: station ? "Computer" : "Phone",
    station,
    channel: "test",
    address: { id: bytes.toString("hex"), addrs: [] },
    signature: sign(null, Buffer.from(`zork-account-device-v1:${session.session_id}:${origin}`), privateKey).toString("hex"),
  };
}
test("account devices require key possession, isolate accounts and disappear on session revocation", async () => {
  const h = await harness();
  try {
    const pc = await h.login("owner"),
      phone = await h.login("owner"),
      outsider = await h.login("other");
    const a = device(pc, true),
      b = device(phone, false),
      c = device(outsider, false);
    const put = (session: Tokens, body: unknown) =>
      h.fetch("/v1/auth/devices", {
        method: "PUT",
        headers: {
          authorization: `Bearer ${session.access_token}`,
          "content-type": "application/json",
        },
        body: JSON.stringify(body),
      });
    assert.equal((await put(pc, a)).status, 200);
    const found = await put(phone, b);
    assert.equal(found.status, 200);
    assert.deepEqual(
      ((await found.json()) as any).devices.map((d: any) => d.origin),
      [a.origin],
    );
    assert.deepEqual(((await (await put(outsider, c)).json()) as any).devices, []);
    assert.equal((await put(phone, { ...a })).status, 401);
    assert.equal((await put(phone, { ...b, signature: "00".repeat(64) })).status, 401);
    assert.equal(
      (
        await h.fetch("/v1/auth/logout", {
          method: "POST",
          headers: {
            authorization: `Bearer ${pc.refresh_token}`,
            "content-type": "application/json",
          },
          body: '{"all":false}',
        })
      ).status,
      200,
    );
    assert.deepEqual(((await (await put(phone, b)).json()) as any).devices, []);
    assert.equal((await put(pc, a)).status, 401);
  } finally {
    await h.close();
  }
});
