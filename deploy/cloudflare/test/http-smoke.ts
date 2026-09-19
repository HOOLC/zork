// Run against wrangler dev or a deployed Worker. Discovery is signed; no device allowlist.
// Usage: pnpm exec tsx test/http-smoke.ts BASE_URL PATH_TO_PROBE_SEED
import assert from "node:assert/strict";
import fs from "node:fs";
import { createPrivateKey, createPublicKey, sign } from "node:crypto";
import dns from "dns-packet";

const [base, seedPath] = process.argv.slice(2);
if (!base || !seedPath) throw new Error("expected BASE_URL PATH_TO_PROBE_SEED");
const privateKey = createPrivateKey({
  key: Buffer.concat([Buffer.from("302e020100300506032b657004220420", "hex"), fs.readFileSync(seedPath)]),
  format: "der",
  type: "pkcs8",
});
const key = createPublicKey(privateKey).export({ format: "der", type: "spki" }).subarray(-32);
const bits = [...key]
  .map((v) => v.toString(2).padStart(8, "0"))
  .join("")
  .padEnd(260, "0");
const alphabet = "ybndrfg8ejkmcpqxot1uwisza345h769";
const z32 = bits
  .match(/.{5}/g)!
  .map((v) => alphabet[parseInt(v, 2)])
  .join("");
const url = `${base}/pkarr/${z32}`;
const start = BigInt(Date.now() - 10_000) * 1000n;
function packet(time: bigint, text: string): Uint8Array<ArrayBuffer> {
  const wire = dns.encode({
    type: "response",
    id: 0,
    answers: [{ name: `_iroh.${z32}`, type: "TXT", ttl: 30, data: text }],
  });
  const timestamp = Buffer.alloc(8);
  timestamp.writeBigUInt64BE(time);
  return new Uint8Array(Buffer.concat([sign(null, Buffer.concat([Buffer.from(`3:seqi${time}e1:v${wire.length}:`), wire]), privateKey), timestamp, wire]));
}
async function put(body: Uint8Array<ArrayBuffer>): Promise<number> {
  const response = await fetch(url, { method: "PUT", body });
  await response.arrayBuffer();
  return response.status;
}
const first = packet(start, "relay=https://first.example.test");
const latest = packet(start + 1n, "relay=https://latest.example.test");
assert.equal(await put(first), 204);
assert.equal(await put(latest), 204);
assert.equal(await put(first), 409, "old record must not replace latest");
assert.equal(await put(latest), 204, "identical retry must succeed");
assert.equal(await put(packet(start + 1n, "different value")), 409);
const forged = latest.slice();
forged[0] ^= 1;
assert.equal(await put(forged), 400);
assert.equal(await put(new Uint8Array(1073)), 400);
const response = await fetch(url);
assert.equal(response.status, 200);
assert.deepEqual(new Uint8Array(await response.arrayBuffer()), latest);
const writes = Array.from({ length: 8 }, (_, i) => packet(start + BigInt(10 + i), `value=${i}`));
await Promise.all(writes.slice().reverse().map(put));
assert.deepEqual(new Uint8Array(await (await fetch(url)).arrayBuffer()), writes.at(-1));
assert.equal((await fetch(`${base}/pkarr/${"y".repeat(52)}`)).status, 404);
assert.equal((await fetch(`${base}/metrics`)).status, 404);
assert.equal((await fetch(`${base}/relay`)).status, 426);
console.log("PASS: signed PUT/GET; relay WebSocket is Google-login gated");
