import assert from "node:assert/strict";
import { generateKeyPairSync, sign } from "node:crypto";
import { test } from "node:test";
import dns from "dns-packet";
import { decodeKey, MAX_AGE_MS, readPayload, verifyPayload } from "../src/pkarr.ts";

const alphabet = "ybndrfg8ejkmcpqxot1uwisza345h769";
export function encodeKey(key: Uint8Array): string {
  let acc = 0,
    bits = 0,
    value = "";
  for (const b of key) {
    acc = (acc << 8) | b;
    bits += 8;
    while (bits >= 5) {
      bits -= 5;
      value += alphabet[(acc >>> bits) & 31];
    }
  }
  if (bits) value += alphabet[(acc << (5 - bits)) & 31];
  return value;
}

function fixture(timestamp = BigInt(Date.now()) * 1000n) {
  const { publicKey, privateKey } = generateKeyPairSync("ed25519");
  const key = new Uint8Array(publicKey.export({ format: "der", type: "spki" }).subarray(-32));
  const packet = dns.encode({
    type: "response",
    id: 0,
    answers: [
      {
        name: `_iroh.${encodeKey(key)}`,
        type: "TXT",
        ttl: 30,
        data: "relay=https://relay.example.test",
      },
    ],
  });
  const preimage = Buffer.concat([Buffer.from(`3:seqi${timestamp}e1:v${packet.length}:`), packet]);
  const time = Buffer.alloc(8);
  time.writeBigUInt64BE(timestamp);
  const payload = new Uint8Array(Buffer.concat([sign(null, preimage, privateKey), time, packet]));
  return { key, payload, timestamp };
}

test("accepts an Ed25519-signed BEP44 packet and canonical z-base-32 key", async () => {
  const f = fixture();
  assert.deepEqual(decodeKey(encodeKey(f.key)), f.key);
  assert.equal(await verifyPayload(f.key, f.payload), f.timestamp);
});

test("rejects forged signatures, changed timestamps, and another identity", async () => {
  const f = fixture();
  const changed = f.payload.slice();
  changed[changed.length - 1] ^= 1;
  await assert.rejects(verifyPayload(f.key, changed));
  const changedTime = f.payload.slice();
  changedTime[71] ^= 1;
  await assert.rejects(verifyPayload(f.key, changedTime));
  await assert.rejects(verifyPayload(fixture().key, f.payload));
});

test("rejects expired or far-future signed records", async () => {
  for (const time of [Date.now() - MAX_AGE_MS - 1000, Date.now() + 600_000]) {
    const f = fixture(BigInt(time) * 1000n);
    await assert.rejects(verifyPayload(f.key, f.payload), /timestamp/);
  }
});

test("rejects malformed keys", () => {
  assert.throws(() => decodeKey("a".repeat(52)));
  assert.throws(() => decodeKey("y".repeat(51) + "b"));
});

test("bounds streaming bodies even without Content-Length", async () => {
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(new Uint8Array(1073));
      c.close();
    },
  });
  const init = { method: "PUT", body: stream, duplex: "half" };
  await assert.rejects(readPayload(new Request("https://test", init)), /large/);
});
