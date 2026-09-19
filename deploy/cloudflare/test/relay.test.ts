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

test("pending upgrades consume capacity before the backend replies", { timeout: 10000 }, async () => {
  const h = await harness({ noGoogle: true });
  try {
    const relays: any = await h.mf.getDurableObjectNamespace("RELAY");
    await relays.get(relays.idFromName("primary")).delay(500);
    const responses = await Promise.all(Array.from({ length: 8 + 1 }, () => h.fetch("/relay", { headers: upgrade })));
    assert.equal(responses.filter((r) => r.status === 101).length, 8);
    assert.equal(responses.filter((r) => r.status === 429).length, 1);
    for (const response of responses) {
      response.webSocket?.accept();
      response.webSocket?.close();
    }
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
