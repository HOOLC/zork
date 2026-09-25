import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { harness } from "./harness.ts";
import { batch, clientAuth, connect, connectRaw, datagram, identity, nothingPending, ping, sign } from "./relay-client.ts";
import { blake3, CHALLENGE_DOMAIN, deriveKey, encodeDeny, encodeRestarting, encodeStatus, headerClaim, hex, negotiate, parseClientFrame, readVarint, relayDatagram, verifyClientAuth } from "../src/relay-protocol.ts";

type H = Awaited<ReturnType<typeof harness>>;
const hub = async (h: H) => {
  const namespace: any = await h.mf.getDurableObjectNamespace("RELAY_HUB");
  return namespace.get(namespace.idFromName("primary"));
};
const bytes = (...values: number[]) => Uint8Array.from(values);
const text = (frame: Uint8Array) => new TextDecoder().decode(frame);

// ---- wire format (no Worker) ----------------------------------------------

test("BLAKE3 and the challenge message match iroh", () => {
  assert.equal(hex(blake3(new Uint8Array(0))), "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262");
  assert.equal(hex(blake3(new TextEncoder().encode("abc"))), "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85");
  // `relay-interop vector` (crates/zork-mesh/examples) prints iroh's value.
  assert.equal(
    hex(
      deriveKey(
        CHALLENGE_DOMAIN,
        Uint8Array.from({ length: 16 }, (_, i) => i),
      ),
    ),
    "b7b16e505d2cb5dbaf494800092c3b05a6c45d8d3fd4f1b888c6a77240b876c0",
  );
});

test("frames encode and decode like iroh-relay 1.1", () => {
  // Snapshot bytes from iroh-relay's own tests (protos/relay.rs, key [42; 32]).
  const key = Buffer.from("197f6b23e16c8532c6abc838facd5ea789be0c76b2920334039bfa8b3d368d61", "hex");
  assert.equal(hex(encodeRestarting(10, 20)), "0c0000000a00000014");
  assert.equal(hex(encodeStatus(2, 1)!), "0d01");
  assert.equal(hex(encodeStatus(2, 2)!), "0d02");
  assert.equal(encodeStatus(1, 2), null, "v1 clients are not told about rate limiting");
  assert.equal(text(encodeStatus(1, 1)!.subarray(1)), "Another endpoint connected with the same endpoint id. No more messages will be received.");
  assert.equal(encodeStatus(1, 1)![0], 11);
  const hello = new TextEncoder().encode("Hello World!");
  const batchIn = batch(key, 6, hello, 3);
  const source = new Uint8Array(32).fill(7);
  assert.equal(hex(relayDatagram(batchIn, key)), "07" + key.toString("hex") + "03" + "0006" + Buffer.from(hello).toString("hex"));
  assert.equal(hex(relayDatagram(datagram(key, hello, 3), key)), "06" + key.toString("hex") + "03" + Buffer.from(hello).toString("hex"));
  assert.deepEqual(relayDatagram(datagram(key, hello), source).subarray(1, 33), source);
  // postcard string: LEB128 length, then UTF-8.
  assert.equal(hex(encodeDeny("signature invalid")), "0311" + Buffer.from("signature invalid").toString("hex"));
  assert.equal(hex(encodeDeny("x".repeat(200)).subarray(0, 3)), "03c801");
  // Parsing.
  assert.deepEqual(readVarint(bytes(0x40, 0x05)), { value: 5, length: 2 });
  assert.equal(readVarint(bytes(0x80, 0, 0)), null);
  assert.equal(parseClientFrame(datagram(key, hello)).kind, "datagram");
  assert.equal(parseClientFrame(batch(key, 6, hello)).kind, "datagram");
  assert.equal(parseClientFrame(bytes(4, ...key)).kind, "invalid", "a datagram needs its ECN byte");
  assert.equal(parseClientFrame(bytes(5, ...key, 0, 0)).kind, "invalid", "a batch needs its segment size");
  assert.equal(parseClientFrame(bytes(0x40, 4, ...key, 0)).kind, "invalid");
  assert.deepEqual(parseClientFrame(ping(bytes(1, 2, 3, 4, 5, 6, 7, 8))), { kind: "ping", data: bytes(1, 2, 3, 4, 5, 6, 7, 8) });
  assert.equal(parseClientFrame(bytes(9, 1)).kind, "invalid");
  assert.equal(parseClientFrame(bytes(6, ...key, 0)).kind, "invalid", "relay-to-client frames are not accepted from clients");
  assert.equal(parseClientFrame(bytes(4, ...key, 0, ...new Uint8Array(64 * 1024))).kind, "invalid", "iroh-relay's MAX_PACKET_SIZE");
  assert.equal(parseClientFrame(new Uint8Array(0)).kind, "invalid");
  // Subprotocol negotiation picks the newest supported version.
  assert.equal(negotiate("iroh-relay-v2, iroh-relay-v1"), 2);
  assert.equal(negotiate("iroh-relay-v1,iroh-relay-v2"), 2);
  assert.equal(negotiate("iroh-relay-v1"), 1);
  assert.equal(negotiate("iroh-relay, v3"), null);
  assert.equal(negotiate(null), null);
});

test("client auth verification uses real ed25519 signatures over the derived challenge", async () => {
  const who = identity(),
    other = identity();
  const challenge = crypto.getRandomValues(new Uint8Array(16));
  const auth = parseClientFrame(clientAuth(who.id, sign(who, challenge)));
  assert.equal(auth.kind, "auth");
  if (auth.kind !== "auth") return;
  assert.equal(await verifyClientAuth(auth.publicKey, auth.signature, challenge), true);
  assert.equal(await verifyClientAuth(auth.publicKey, auth.signature, crypto.getRandomValues(new Uint8Array(16))), false, "another challenge");
  assert.equal(await verifyClientAuth(other.id, auth.signature, challenge), false, "another key");
  assert.equal(await verifyClientAuth(new Uint8Array(32).fill(0xff), auth.signature, challenge), false, "not a curve point");
  const header = Buffer.from(Uint8Array.of(...who.id, 64, ...new Uint8Array(80))).toString("base64url");
  assert.deepEqual(headerClaim(header), who.id);
  assert.equal(headerClaim("garbage"), null);
  assert.equal(headerClaim(null), null);
});

// ---- Worker + Durable Object --------------------------------------------------

test("net report captive-portal probe gets iroh-relay's 204 challenge response", async () => {
  const h = await harness({ noGoogle: true });
  try {
    const ok = await h.fetch("/generate_204", { headers: { "x-iroh-challenge": "ts1727.ab-C_9" } });
    assert.equal(ok.status, 204);
    assert.equal(ok.headers.get("x-iroh-response"), "response ts1727.ab-C_9");
    const plain = await h.fetch("/generate_204");
    assert.equal(plain.status, 204);
    assert.equal(plain.headers.get("x-iroh-response"), null);
    const bad = await h.fetch("/generate_204", { headers: { "x-iroh-challenge": "a b<script>" } });
    assert.equal(bad.status, 204);
    assert.equal(bad.headers.get("x-iroh-response"), null, "invalid challenges are not echoed");
  } finally {
    await h.close();
  }
});

test("upgrade negotiation, signed challenge and ServerConfirmsAuth; operator restart", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    assert.equal((await h.fetch("/relay")).status, 426);
    assert.equal((await h.fetch("/relay", { method: "POST" })).status, 405);
    assert.equal((await h.mf.dispatchFetch("https://wrong.example/relay", { headers: { upgrade: "websocket", "sec-websocket-protocol": "iroh-relay-v2" } })).status, 421);
    assert.equal((await connectRaw(h, { protocol: "iroh-relay" })).status, 400, "unsupported subprotocols are refused like iroh-relay");
    const v1 = await connectRaw(h, { protocol: "iroh-relay-v1" });
    assert.equal(v1.status, 101);
    if (v1.status === 101) {
      assert.equal(v1.protocol, "iroh-relay-v1");
      v1.close();
    }
    const client = await connect(h, identity(), { header: true });
    assert.equal(client.protocol, "iroh-relay-v2");
    assert.ok(await nothingPending(client), "Ping is answered with the same Pong payload");
    // Cloud credentials never matter to the relay; only an operator can restart it.
    const session = await h.fetch("/v1/admin/relay/restart", { method: "POST", headers: { authorization: "Bearer invalid" } });
    assert.equal(session.status, 401);
    const restart = await h.fetch("/v1/admin/relay/restart", { method: "POST", headers: { authorization: "Bearer " + h.adminToken } });
    assert.deepEqual(await restart.json(), { restarted: true, connections: 1 });
    const restarting = await client.next();
    assert.equal(restarting[0], 12);
    assert.equal(restarting.length, 9);
    assert.equal(await client.closed, 1012);
  } finally {
    await h.close();
  }
});

test("wrong signatures and mismatched identity headers are denied without charging the identity", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const who = identity(),
      impostor = identity();
    for (const [label, headers, signer] of [
      ["signature over another challenge", {}, who],
      ["signature by another key", {}, impostor],
    ] as const) {
      const c = await connectRaw(h);
      assert.equal(c.status, 101);
      if (c.status !== 101) return;
      const challenge = await c.next();
      const wrong = label.includes("challenge") ? crypto.getRandomValues(new Uint8Array(16)) : challenge.subarray(1);
      c.send(clientAuth(who.id, sign(signer, wrong)));
      const denial = await c.next();
      assert.equal(denial[0], 3, label);
      assert.equal(text(denial.subarray(2)), "signature invalid");
      assert.equal(await c.closed, 1008);
      void headers;
    }
    // A header naming someone else's EndpointId is only a routing hint; the
    // proven key must match it.
    const c = await connectRaw(h, { headers: { "x-iroh-relay-client-auth-v1": Buffer.from(Uint8Array.of(...impostor.id, 64, ...new Uint8Array(80))).toString("base64url") } });
    if (c.status !== 101) throw new Error("upgrade");
    const challenge = await c.next();
    c.send(clientAuth(who.id, sign(who, challenge.subarray(1))));
    assert.equal((await c.next())[0], 3);
    assert.equal(await c.closed, 1008);
    // Anything but ClientAuth during the handshake ends the connection.
    const early = await connectRaw(h);
    if (early.status !== 101) throw new Error("upgrade");
    await early.next();
    early.send(ping(new Uint8Array(8)));
    assert.equal(await early.closed, 1002);
    const stats = await (await hub(h)).statistics("ep:" + hex(who.id));
    assert.equal(stats.connects, 0, "denied handshakes never charge the claimed identity");
  } finally {
    await h.close();
  }
});

test("datagrams are forwarded by EndpointId with ECN and batch fields intact; unknown destinations are dropped", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const a = await connect(h, identity(), { header: true });
    const b = await connect(h); // no header: found without a routing tag
    const payload = crypto.getRandomValues(new Uint8Array(1200));
    a.send(datagram(b.id, payload, 2));
    assert.equal(hex(await b.next()), "06" + hex(a.id) + "02" + hex(payload));
    const segments = crypto.getRandomValues(new Uint8Array(3 * 1000));
    b.send(batch(a.id, 1000, segments, 3));
    assert.equal(hex(await a.next()), "07" + hex(b.id) + "03" + "03e8" + hex(segments));
    const maximum = crypto.getRandomValues(new Uint8Array(64 * 1024 - 32 - 1));
    a.send(datagram(b.id, maximum));
    assert.equal((await b.next()).length, 1 + 64 * 1024);
    a.send(datagram(identity().id, payload));
    assert.ok(await nothingPending(a), "no error or notice for an offline destination (iroh-relay drops it)");
    assert.ok(await nothingPending(b));
    // Frames only the relay may send end the connection.
    b.send(Uint8Array.of(6, ...a.id, 0));
    assert.equal(await b.closed, 1002);
    a.close();
  } finally {
    await h.close();
  }
});

test("EndpointGone reaches only the peers the departed endpoint sent to", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const a = await connect(h, identity(), { header: true });
    const b = await connect(h, identity(), { header: true });
    const c = await connect(h, identity(), { header: true });
    a.send(datagram(b.id, bytes(1)));
    await b.next();
    c.send(datagram(a.id, bytes(2)));
    await a.next();
    a.close();
    assert.equal(hex(await b.next()), "08" + hex(a.id));
    assert.ok(await nothingPending(c), "c only received from a, so it gets no EndpointGone");
  } finally {
    await h.close();
  }
});

test("duplicate EndpointId: newest wins, the displaced connection is told and promoted back", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const who = identity();
    const a = await connect(h, identity(), { header: true });
    const b1 = await connect(h, who, { header: true, protocol: "iroh-relay-v1" });
    const b2 = await connect(h, who, { header: true });
    const notice = await b1.next();
    assert.equal(notice[0], 11, "v1 clients get the status as Health text");
    assert.match(text(notice.subarray(1)), /same endpoint id/);
    a.send(datagram(who.id, bytes(1, 2, 3)));
    assert.equal(hex(await b2.next()), "06" + hex(a.id) + "00010203");
    assert.ok(await nothingPending(b1), "the displaced connection stays open but receives nothing");
    // The inactive connection may still send, as in iroh-relay.
    b1.send(datagram(a.id, bytes(9)));
    assert.equal(hex(await a.next()), "06" + hex(who.id) + "0009");
    const b3 = await connect(h, who, { header: true });
    assert.equal(hex(await b2.next()), "0d01", "v2 clients get Status(SameEndpointIdConnected)");
    b3.close();
    assert.equal(hex(await b2.next()), "0d00", "the newest remaining connection is promoted: Status(Healthy)");
    a.send(datagram(who.id, bytes(4)));
    assert.equal(hex(await b2.next()), "06" + hex(a.id) + "0004");
    b2.close();
    const healthy = await b1.next();
    assert.equal(healthy[0], 11);
    a.send(datagram(who.id, bytes(5)));
    assert.equal(hex(await b1.next()), "06" + hex(a.id) + "0005");
    assert.ok(await nothingPending(a), "no EndpointGone while a connection of the endpoint remains");
    b1.close();
    assert.equal(hex(await a.next()), "08" + hex(who.id), "EndpointGone once the last connection left");
  } finally {
    await h.close();
  }
});

test("hibernation: routing, duplicates and EndpointGone survive losing all in-memory state", { timeout: 20000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relay = await hub(h);
    const who = identity();
    const a = await connect(h, identity(), { header: true });
    const b1 = await connect(h, who, { header: true });
    const anon = await connect(h);
    a.send(datagram(who.id, bytes(1)));
    await b1.next();
    await relay.forgetMemory();
    a.send(datagram(who.id, bytes(2)));
    assert.equal(hex(await b1.next()), "06" + hex(a.id) + "0002", "routing from socket tags and attachments");
    a.send(datagram(anon.id, bytes(3)));
    assert.equal((await anon.next())[0], 6, "header-less connections are found after eviction too");
    await relay.forgetMemory();
    const b2 = await connect(h, who, { header: true });
    assert.equal(hex(await b1.next()), "0d01", "connection order is persisted");
    a.send(datagram(who.id, bytes(4)));
    assert.equal(hex(await b2.next()), "06" + hex(a.id) + "0004");
    await relay.forgetMemory();
    b2.close();
    assert.equal(hex(await b1.next()), "0d00");
    await relay.forgetMemory();
    b1.send(datagram(a.id, bytes(5)));
    await a.next();
    await relay.forgetMemory();
    b1.close();
    // b2's sends were merged into b1 when it closed; a learns once the endpoint is gone.
    assert.equal(hex(await a.next()), "08" + hex(who.id));
    assert.deepEqual(await relay.endpoints(hex(who.id)), []);
  } finally {
    await h.close();
  }
});

test("counters are written sparingly: significant movement within seconds, the rest at the sweep", { timeout: 20000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relay = await hub(h);
    const a = await connect(h, identity(), { ip: "198.51.100.5" });
    for (let i = 0; i < 5; i++) assert.ok(await nothingPending(a));
    assert.equal(await relay.persisted("ip:198.51.100.5"), null, "a few pings cause no storage write");
    await relay.sweep();
    assert.equal((await relay.persisted("ip:198.51.100.5")).frames, 6, "the sweep writes everything (auth + 5 pings)");
    for (let i = 0; i < 1100; i++) a.send(ping(new Uint8Array(8)));
    for (let i = 0; i < 1100; i++) await a.next();
    const deadline = Date.now() + 9000;
    let persisted: any;
    while (Date.now() < deadline) {
      persisted = await relay.persisted("ip:198.51.100.5");
      if (persisted.frames >= 1106) break;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
    assert.equal(persisted.frames, 1106, "an alarm persists significant movement before the object could hibernate");
    await relay.forgetMemory();
    assert.equal((await relay.statistics("ip:198.51.100.5")).frames, 1106, "counters reload from storage after eviction");
  } finally {
    await h.close();
  }
});

test("pending handshakes time out; rate bursts drop frames and notify once", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relay = await hub(h);
    const idle = await connectRaw(h);
    if (idle.status !== 101) throw new Error("upgrade");
    await idle.next();
    await relay.ageHandshakes(31);
    await relay.sweep();
    // Miniflare does not deliver a server close to a test socket that never sent
    // anything, so check the server side (wrangler dev interop checks the client side).
    assert.deepEqual(await relay.debugSockets(), [], "the pending handshake was closed");
    const a = await connect(h, identity(), { ip: "198.51.100.8" });
    const b = await connect(h, identity(), { ip: "203.0.113.8" });
    await relay.drainRate("ip:198.51.100.8");
    a.send(datagram(b.id, bytes(1)));
    assert.equal(hex(await a.next()), "0d02", "Status(RateLimited) once");
    a.send(datagram(b.id, bytes(2)));
    assert.ok(await nothingPending(b), "dropped, not forwarded");
    await new Promise((resolve) => setTimeout(resolve, 2100));
    a.send(datagram(b.id, bytes(3)));
    assert.equal(hex(await b.next()), "06" + hex(a.id) + "0003", "the bucket refills; the connection was kept");
    assert.ok(await nothingPending(a), "RateLimited is not repeated");
  } finally {
    await h.close();
  }
});

// ---- budgets -------------------------------------------------------------------

test("per-IP concurrency is bounded before authentication; other clients are unaffected", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const opened = [];
    for (let i = 0; i < 32; i++) opened.push(await connectRaw(h, { ip: "198.51.100.7" }));
    assert.ok(opened.every((c) => c.status === 101));
    const rejected = await connectRaw(h, { ip: "198.51.100.7" });
    assert.equal(rejected.status, 429);
    assert.equal(rejected.response.headers.get("retry-after"), "30");
    const other = await connect(h, identity(), { ip: "203.0.113.9" });
    assert.ok(await nothingPending(other), "another client keeps connecting while one IP is at its limit");
    for (const c of opened) if (c.status === 101) c.close();
  } finally {
    await h.close();
  }
});

test("connect rate is limited per IP with Retry-After; IPv6 clients share their /64", { timeout: 20000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    for (let i = 0; i < 60; i++) {
      const c = await connectRaw(h, { ip: `2001:db8:0:1:${i.toString(16)}::1` });
      assert.equal(c.status, 101);
      if (c.status === 101) c.close();
    }
    const limitedResponse = await connectRaw(h, { ip: "2001:db8::1:ffff:0:0:1" });
    assert.equal(limitedResponse.status, 429);
    const retry = Number(limitedResponse.response.headers.get("retry-after"));
    assert.ok(retry >= 1 && retry <= 60, "retry after the minute window: " + retry);
    for (const ip of ["2001:db8:0:2::1", "192.0.2.1"]) {
      const c = await connectRaw(h, { ip });
      assert.equal(c.status, 101);
      if (c.status === 101) c.close();
    }
  } finally {
    await h.close();
  }
});

test("an exhausted client budget closes only that client's connections", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const noisy = [await connect(h, identity(), { ip: "198.51.100.7" }), await connect(h, identity(), { ip: "198.51.100.7" })];
    const quiet = await connect(h, identity(), { ip: "203.0.113.9" });
    await (await hub(h)).exhaustBudget("bytes", "ip:198.51.100.7");
    noisy[0].send(ping(new Uint8Array(8)));
    assert.deepEqual(await Promise.all(noisy.map((c) => c.closed)), [4008, 4008]);
    assert.ok(await nothingPending(quiet));
    const denied = await connectRaw(h, { ip: "198.51.100.7" });
    assert.equal(denied.status, 429);
    assert.ok(Number(denied.response.headers.get("retry-after")) > 60, "a used-up day retries after the UTC day ends");
    (await connect(h, identity(), { ip: "203.0.113.10" })).close();
  } finally {
    await h.close();
  }
});

test("verified endpoint identities get their own connection budget", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const who = identity();
    const admitted = [];
    for (let i = 0; i < 4; i++) admitted.push(await connect(h, who, { ip: `198.51.100.${i}`, header: true }));
    const extra = await connectRaw(h, { ip: "198.51.100.200" });
    if (extra.status !== 101) throw new Error("upgrade");
    const challenge = await extra.next();
    extra.send(clientAuth(who.id, sign(who, challenge.subarray(1))));
    assert.equal(text((await extra.next()).subarray(2)), "relay quota");
    assert.equal(await extra.closed, 4008, "a fifth connection of one endpoint is refused after its signature");
    const other = await connect(h, identity(), { ip: "198.51.100.200" });
    assert.ok(await nothingPending(other), "another endpoint on the same IP is admitted");
    await (await hub(h)).exhaustBudget("bytes", "ep:" + hex(who.id));
    admitted[0].send(ping(new Uint8Array(8)));
    assert.deepEqual(await Promise.all(admitted.map((c) => c.closed)), [4008, 4008, 4008, 4008]);
    assert.ok(await nothingPending(other));
  } finally {
    await h.close();
  }
});

test("the global safety cap still bounds the whole service", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relay = await hub(h);
    const sockets = [await connect(h, identity(), { ip: "198.51.100.1" }), await connect(h, identity(), { ip: "203.0.113.1" })];
    const pending = await connectRaw(h, { ip: "203.0.113.2" });
    assert.equal(pending.status, 101);
    await relay.exhaustBudget("frames");
    sockets[0].send(new Uint8Array(0));
    assert.deepEqual(await Promise.all(sockets.map((c) => c.closed)), [4008, 4008]);
    assert.deepEqual(await relay.debugSockets(), [], "pending handshakes are closed too");
    assert.equal((await connectRaw(h, { ip: "192.0.2.77" })).status, 429);
  } finally {
    await h.close();
  }
});

test("text and oversized frames close the offending connection", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    for (const payload of ["text", new Uint8Array(128 * 1024 + 1)]) {
      const c = await connect(h);
      c.send(payload);
      assert.equal(await c.closed, 1009);
    }
  } finally {
    await h.close();
  }
});

test("a Worker restart cannot reset the anonymous relay byte budget", { timeout: 10000 }, async () => {
  const persist = await fs.mkdtemp(path.join(os.tmpdir(), "zork-relay-budget-"));
  let h = await harness({ noGoogle: true, persist });
  try {
    await (await hub(h)).exhaustBudget();
    await h.close();
    h = await harness({ noGoogle: true, persist });
    assert.equal((await connectRaw(h)).status, 429);
  } finally {
    await h.close();
    await fs.rm(persist, { recursive: true, force: true });
  }
});
