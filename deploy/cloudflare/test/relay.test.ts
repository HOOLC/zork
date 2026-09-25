import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { harness } from "./harness.ts";

const upgrade = { upgrade: "websocket", "sec-websocket-protocol": "iroh-relay" };

test("native relay works without Google configuration or credentials", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    assert.equal((await h.fetch("/v1/auth/session")).status, 503);
    assert.equal((await h.fetch("/v1/admin/relay/restart", { method: "POST", headers: { authorization: "Bearer " + h.adminToken } })).status, 200);
    assert.equal((await h.fetch("/relay")).status, 426);
    assert.equal((await h.fetch("/relay", { method: "POST" })).status, 405);
    assert.equal((await h.mf.dispatchFetch("https://wrong.example/relay", { headers: upgrade })).status, 421);
    const response = await h.fetch("/relay", { headers: upgrade });
    assert.equal(response.status, 101);
    const socket = response.webSocket!;
    socket.accept();
    const echoed = new Promise<unknown>((resolve) => socket.addEventListener("message", (e) => resolve(e.data), { once: true }));
    socket.send(new Uint8Array([10, 20, 30]));
    assert.deepEqual(new Uint8Array((await echoed) as ArrayBuffer), new Uint8Array([10, 20, 30]));
    socket.close();
  } finally {
    await h.close();
  }
});

type Socket = NonNullable<Awaited<ReturnType<Awaited<ReturnType<typeof harness>>["fetch"]>>["webSocket"]>;
const from = (ip: string) => ({ ...upgrade, "cf-connecting-ip": ip });
const open = async (h: Awaited<ReturnType<typeof harness>>, ip: string) => {
  const response = await h.fetch("/relay", { headers: from(ip) });
  assert.equal(response.status, 101);
  const socket = response.webSocket!;
  socket.accept();
  return socket;
};
const closeCode = (socket: Socket) => new Promise<number>((resolve) => socket.addEventListener("close", (e) => resolve(e.code), { once: true }));
const nextFrame = (socket: Socket) => new Promise<Uint8Array>((resolve) => socket.addEventListener("message", (e) => resolve(new Uint8Array(e.data as ArrayBuffer)), { once: true }));

test("pending upgrades consume per-IP capacity before the backend replies; other clients are unaffected", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relays: any = await h.mf.getDurableObjectNamespace("RELAY");
    await relays.get(relays.idFromName("primary")).delay(500);
    const responses = await Promise.all(Array.from({ length: 32 + 1 }, () => h.fetch("/relay", { headers: from("198.51.100.7") })));
    assert.equal(responses.filter((r) => r.status === 101).length, 32);
    const rejected = responses.filter((r) => r.status === 429);
    assert.equal(rejected.length, 1);
    assert.equal(rejected[0].headers.get("retry-after"), "30");
    const other = await h.fetch("/relay", { headers: from("203.0.113.9") });
    assert.equal(other.status, 101, "another client keeps connecting while one IP is at its limit");
    for (const response of [...responses, other]) {
      response.webSocket?.accept();
      response.webSocket?.close();
    }
  } finally {
    await h.close();
  }
});

test("connect rate is limited per IP with Retry-After; IPv6 clients share their /64", { timeout: 20000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const sockets: Socket[] = [];
    for (let i = 0; i < 60; i++) {
      const socket = await open(h, `2001:db8:0:1:${i.toString(16)}::1`);
      socket.close();
      sockets.push(socket);
    }
    const limitedResponse = await h.fetch("/relay", { headers: from("2001:db8::1:ffff:0:0:1") });
    assert.equal(limitedResponse.status, 429);
    const retry = Number(limitedResponse.headers.get("retry-after"));
    assert.ok(retry >= 1 && retry <= 60, "retry after the minute window: " + retry);
    (await open(h, "2001:db8:0:2::1")).close();
    (await open(h, "192.0.2.1")).close();
  } finally {
    await h.close();
  }
});

test("an exhausted client budget closes only that client's connections", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const noisy = [await open(h, "198.51.100.7"), await open(h, "198.51.100.7")];
    const quiet = await open(h, "203.0.113.9");
    const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_BUDGET");
    await budgets.get(budgets.idFromName("primary")).exhaustBudget("bytes", "ip:198.51.100.7");
    const closes = noisy.map(closeCode);
    noisy[0].send(new Uint8Array([1]));
    assert.deepEqual(await Promise.all(closes), [4008, 4008]);
    const echoed = nextFrame(quiet);
    quiet.send(new Uint8Array([7, 8]));
    assert.deepEqual(await echoed, new Uint8Array([7, 8]));
    const denied = await h.fetch("/relay", { headers: from("198.51.100.7") });
    assert.equal(denied.status, 429);
    assert.ok(Number(denied.headers.get("retry-after")) > 60, "a used-up day retries after the UTC day ends");
    (await open(h, "203.0.113.10")).close();
    quiet.close();
  } finally {
    await h.close();
  }
});

test("verified endpoint identities get their own connection budget", { timeout: 15000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relays: any = await h.mf.getDurableObjectNamespace("RELAY");
    await relays.get(relays.idFromName("primary")).useHandshake(true);
    const auth = (key: number) => new Uint8Array([1, ...new Uint8Array(32).fill(key), 64, ...new Uint8Array(64)]);
    const handshake = async (ip: string, key: number) => {
      const socket = await open(h, ip);
      const challenge = await nextFrame(socket);
      assert.equal(challenge[0], 0);
      const reply = Promise.race([nextFrame(socket), closeCode(socket).then((code) => code)]);
      socket.send(auth(key));
      return { socket, reply: await reply };
    };
    const admitted = [];
    for (let i = 0; i < 4; i++) {
      const { socket, reply } = await handshake(`198.51.100.${i}`, 0x11);
      assert.deepEqual(reply, new Uint8Array([2]));
      admitted.push(socket);
    }
    const extra = await handshake("198.51.100.200", 0x11);
    assert.equal(extra.reply, 4008, "a fifth connection of one endpoint is closed before confirmation");
    const other = await handshake("198.51.100.200", 0x22);
    assert.deepEqual(other.reply, new Uint8Array([2]), "another endpoint on the same IP is admitted");
    // A denied handshake never charges the claimed identity.
    const forged = await handshake("198.51.100.201", 0xff);
    assert.deepEqual(forged.reply, new Uint8Array([3]));
    const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_BUDGET");
    assert.equal((await budgets.get(budgets.idFromName("primary")).statistics("ep:" + "ff".repeat(32))).connects, 0);
    // Exhausting one endpoint closes its connections only.
    await budgets.get(budgets.idFromName("primary")).exhaustBudget("bytes", "ep:" + "11".repeat(32));
    const closes = admitted.map(closeCode);
    admitted[0].send(new Uint8Array([9]));
    assert.deepEqual(await Promise.all(closes), [4008, 4008, 4008, 4008]);
    const echoed = nextFrame(other.socket);
    other.socket.send(new Uint8Array([9]));
    assert.deepEqual(await echoed, new Uint8Array([9]));
    other.socket.close();
    forged.socket.close();
  } finally {
    await h.close();
  }
});

test("the global safety cap still bounds the whole service", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const sockets = [await open(h, "198.51.100.1"), await open(h, "203.0.113.1")];
    const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_BUDGET");
    await budgets.get(budgets.idFromName("primary")).exhaustBudget("frames");
    const closes = sockets.map(closeCode);
    sockets[0].send(new Uint8Array(0));
    assert.deepEqual(await Promise.all(closes), [4008, 4008]);
    assert.equal((await h.fetch("/relay", { headers: from("192.0.2.77") })).status, 429);
  } finally {
    await h.close();
  }
});

test("text and oversized frames close the offending connection", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    for (const payload of ["text", new Uint8Array(128 * 1024 + 1)]) {
      const response = await h.fetch("/relay", { headers: upgrade });
      assert.equal(response.status, 101);
      const socket = response.webSocket!;
      socket.accept();
      const closed = new Promise<number>((resolve) => socket.addEventListener("close", (e) => resolve(e.code), { once: true }));
      socket.send(payload);
      assert.equal(await closed, 1009);
    }
  } finally {
    await h.close();
  }
});

test("a Worker restart cannot reset the anonymous relay byte budget", { timeout: 10000 }, async () => {
  const persist = await fs.mkdtemp(path.join(os.tmpdir(), "zork-relay-budget-"));
  let h = await harness({ noGoogle: true, persist });
  try {
    const budgets: any = await h.mf.getDurableObjectNamespace("RELAY_BUDGET");
    await budgets.get(budgets.idFromName("primary")).exhaustBudget();
    await h.close();
    h = await harness({ noGoogle: true, persist });
    assert.equal((await h.fetch("/relay", { headers: upgrade })).status, 429);
  } finally {
    await h.close();
    await fs.rm(persist, { recursive: true, force: true });
  }
});
