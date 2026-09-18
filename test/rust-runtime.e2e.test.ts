import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import fs from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vite-plus/test";

import { brokerRoot, getFreePort, removeTempRoot, stopChild, waitForReady, writeConfig } from "./helpers.js";

describe.sequential("rust runtime", () => {
  const connectionId = "01J00000000000000000000RUN";
  const cleanups: Array<() => Promise<void>> = [];

  afterEach(async () => {
    while (cleanups.length > 0) {
      await cleanups.pop()?.();
    }
  });

  it("serves readyz and rejects provider calls that are not bound to a Session", async () => {
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "runtime-e2e-"));
    cleanups.push(async () => removeTempRoot(tempRoot));
    const dataRoot = path.join(tempRoot, "data");

    const posts: Array<{ url: string; body: string }> = [];
    const stationPort = await getFreePort();
    const station = http.createServer((request, response) => {
      const url = new URL(request.url ?? "/", "http://127.0.0.1");
      if (request.method === "POST" && url.pathname === "/api/auth.test") {
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify({ ok: true, user_id: "UBOT", user: "zork" }));
        return;
      }
      const chunks: Buffer[] = [];
      request.on("data", (chunk) => chunks.push(chunk as Buffer));
      request.on("end", () => {
        posts.push({ url: url.pathname, body: Buffer.concat(chunks).toString() });
        response.setHeader("content-type", "application/json");
        response.end(JSON.stringify({ ok: true, ts: "1.2" }));
      });
    });
    await new Promise<void>((resolve) => station.listen(stationPort, "127.0.0.1", resolve));
    cleanups.push(
      async () =>
        await new Promise<void>((resolve, reject) => {
          station.close((error) => (error ? reject(error) : resolve()));
        }),
    );

    const runtimePort = await getFreePort();
    const controlPort = await getFreePort();
    const publicStationPort = await getFreePort();
    await writeConfig(dataRoot, {
      bind: {
        runtime: `127.0.0.1:${runtimePort}`,
        station: `127.0.0.1:${publicStationPort}`,
        control: `127.0.0.1:${controlPort}`,
      },
      im_connections: [
        {
          id: connectionId,
          name: "Runtime Test Slack",
          provider: "slack",
          enabled: true,
          mode: "normal",
          app_token: "xapp-test",
          bot_token: "xoxb-test",
          api_base_url: `http://127.0.0.1:${stationPort}/api`,
        },
      ],
    });
    const child = spawnRuntime({
      cwd: brokerRoot,
      args: ["--data", dataRoot, "--fake-agent"],
      env: { RUST_LOG: "info" },
    });
    cleanups.push(async () => stopChild(child));
    await waitForReady(`http://127.0.0.1:${runtimePort}/readyz`);

    const ready = await fetch(`http://127.0.0.1:${runtimePort}/readyz`);
    expect(ready.status).toBe(200);
    await expect(ready.json()).resolves.toMatchObject({ ok: true, service: "zork-station" });

    const snapshot = await fetch(`http://127.0.0.1:${runtimePort}/internal/realtime/snapshot`);
    expect(snapshot.status).toBe(200);
    await expect(snapshot.json()).resolves.toMatchObject({ state: { sessions: [] } });

    expect(posts.some((entry) => entry.url.includes("chat.postMessage"))).toBe(false);

    const removedConnectionSelectedApi = await fetch(`http://127.0.0.1:${publicStationPort}/im/${connectionId}/slack/chat.postMessage`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ channel: "C123", thread_ts: "100.200", text: "hello from test" }),
    });
    expect(removedConnectionSelectedApi.status).toBe(404);

    const unknownSessionApi = await fetch(`http://127.0.0.1:${publicStationPort}/sessions/${encodeURIComponent(`${connectionId}:C123:100.200`)}/im/raw/chat.postMessage`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ channel: "C123", thread_ts: "100.200", text: "你好，这是中文回复 **加粗**" }),
    });
    expect(unknownSessionApi.status).toBe(404);
    expect(posts.some((entry) => entry.url.includes("chat.postMessage"))).toBe(false);

    const stillUp = await fetch(`http://127.0.0.1:${runtimePort}/readyz`);
    expect(stillUp.status).toBe(200);

    expect(existsSync(path.join(dataRoot, "bin/zork-call"))).toBe(false);
    expect(existsSync(path.join(dataRoot, "bin/gh"))).toBe(true);
    const rejected = await fetch(`http://127.0.0.1:${runtimePort}/chat/post-message`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ sessionKey: `${connectionId}:C123:100.200`, conversationId: "C123", rootMessageId: "100.200", text: "unknown session", kind: "progress" }),
    });
    expect(rejected.status).toBe(404);
  }, 60_000);
});

function spawnRuntime(options: { readonly cwd: string; readonly args: readonly string[]; readonly env: Record<string, string> }): ChildProcess {
  const binary = path.join(process.env.ZORK_TEST_BIN_DIR ?? path.join(options.cwd, "target/debug"), "zork-station");
  if (!existsSync(binary)) {
    throw new Error(`zork-station missing at ${binary}; run pnpm build:rust first`);
  }
  return spawn(binary, [...options.args], {
    cwd: options.cwd,
    env: {
      ...process.env,
      ...options.env,
    },
    stdio: ["ignore", "inherit", "inherit"],
  });
}
