import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vite-plus/test";

import { brokerRoot, getFreePort, removeTempRoot, spawnBinary, stopChild, waitForReady, writeConfig } from "./helpers.js";

describe.sequential("admin plane (in-process)", () => {
  const cleanups: Array<() => Promise<void>> = [];

  afterEach(async () => {
    while (cleanups.length > 0) {
      await cleanups.pop()?.();
    }
  });

  it("serves admin APIs and realtime session data from the same process", async () => {
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "admin-e2e-"));
    cleanups.push(async () => removeTempRoot(tempRoot));
    const dataRoot = tempRoot;
    await fs.mkdir(path.join(dataRoot, "ui", "assets"), { recursive: true });
    await fs.writeFile(path.join(dataRoot, "ui", "index.html"), "legacy admin page");
    await fs.writeFile(path.join(dataRoot, "ui", "assets", "main.js"), "legacy admin script");

    const [stationPort, runtimePort, controlPort] = await Promise.all([getFreePort(), getFreePort(), getFreePort()]);
    await writeConfig(dataRoot, {
      bind: {
        station: `127.0.0.1:${stationPort}`,
        runtime: `127.0.0.1:${runtimePort}`,
        control: `127.0.0.1:${controlPort}`,
      },
    });
    const child = spawnBinary("zork-station", {
      cwd: brokerRoot,
      args: ["--data", dataRoot, "--fake-agent"],
      env: { RUST_LOG: "info" },
    });
    cleanups.push(async () => stopChild(child));
    await waitForReady(`http://127.0.0.1:${controlPort}/readyz`);
    await waitForReady(`http://127.0.0.1:${runtimePort}/readyz`);

    const ready = await fetch(`http://127.0.0.1:${controlPort}/readyz`);
    expect(ready.status).toBe(200);
    await expect(ready.json()).resolves.toMatchObject({ ok: true, service: "zork-station" });

    for (const route of ["/", "/admin", "/admin/", "/admin/sessions/old-session", "/admin/assets/main.js", "/admin/api/missing"]) {
      const response = await fetch(`http://127.0.0.1:${controlPort}${route}`, { redirect: "manual" });
      expect(response.status).toBe(404);
      await expect(response.json()).resolves.toEqual({ ok: false, error: "not_found" });
    }

    // Realtime data flows through the admin plane without an HTTP hop.
    const sessions = await fetch(`http://127.0.0.1:${controlPort}/admin/api/sessions`);
    expect(sessions.status).toBe(200);
    await expect(sessions.json()).resolves.toMatchObject({ ok: true });

    const overview = await fetch(`http://127.0.0.1:${controlPort}/admin/api/overview`);
    expect(overview.status).toBe(200);
    await expect(overview.json()).resolves.toMatchObject({
      ok: true,
      service: { name: "zork-station", mode: "single" },
    });

    // Connection creation persists one explicit IM instance and never returns credentials.
    const saved = await fetch(`http://127.0.0.1:${controlPort}/admin/api/im/connections`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        name: "测试 Slack",
        provider: "slack",
        mode: "proactive",
        enabled: false,
        appToken: "xapp-test",
        botToken: "xoxb-test",
      }),
    });
    expect(saved.status).toBe(201);
    const savedResponse = await saved.json();
    expect(savedResponse.connection).toMatchObject({
      name: "测试 Slack",
      provider: "slack",
      mode: "proactive",
      fieldValues: { apiBaseUrl: "https://slack.com/api" },
      secretFieldsSet: { appToken: true, botToken: true },
    });
    expect(savedResponse.connection).not.toHaveProperty("appTokenSet");
    expect(savedResponse.connection).not.toHaveProperty("botTokenSet");
    expect(JSON.stringify(savedResponse)).not.toContain("xapp-test");
    expect(JSON.stringify(savedResponse)).not.toContain("xoxb-test");
    const file = JSON.parse(await fs.readFile(path.join(dataRoot, "config.json"), "utf8")) as {
      im_connections?: Array<{ mode?: string; app_token?: string; bot_token?: string }>;
    };
    expect(file.im_connections?.[0]?.mode).toBe("proactive");
    expect(file.im_connections?.[0]?.app_token).toBe("xapp-test");
    expect(file.im_connections?.[0]?.bot_token).toBe("xoxb-test");

    // Reload without a supervisor answers an error, not a crash.
    const reload = await fetch(`http://127.0.0.1:${controlPort}/admin/api/reload`, {
      method: "POST",
    });
    expect([200, 500, 502]).toContain(reload.status);
    // Process stays alive after a failed reload attempt.
    const still = await fetch(`http://127.0.0.1:${controlPort}/readyz`);
    expect(still.status).toBe(200);
  }, 60_000);

  it("manages multiple IM connections of the same provider with independent modes", async () => {
    const tempRoot = await fs.mkdtemp(path.join(os.tmpdir(), "control-im-connections-"));
    cleanups.push(async () => removeTempRoot(tempRoot));
    const [stationPort, runtimePort, controlPort] = await Promise.all([getFreePort(), getFreePort(), getFreePort()]);
    await writeConfig(tempRoot, {
      bind: {
        station: `127.0.0.1:${stationPort}`,
        runtime: `127.0.0.1:${runtimePort}`,
        control: `127.0.0.1:${controlPort}`,
      },
    });
    const child = spawnBinary("zork-station", {
      cwd: brokerRoot,
      args: ["--data", tempRoot, "--fake-agent"],
    });
    cleanups.push(async () => stopChild(child));
    await waitForReady(`http://127.0.0.1:${controlPort}/readyz`);

    const create = async (name: string, mode: "normal" | "proactive", suffix: string) => {
      const response = await fetch(`http://127.0.0.1:${controlPort}/admin/api/im/connections`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          name,
          provider: "slack",
          mode,
          enabled: false,
          appToken: `xapp-${suffix}`,
          botToken: `xoxb-${suffix}`,
        }),
      });
      expect(response.status).toBe(201);
      return (await response.json()) as Record<string, any>;
    };

    const normal = await create("工作 Slack", "normal", "work");
    const proactive = await create("社区 Slack", "proactive", "community");
    expect(normal.connection).toMatchObject({ name: "工作 Slack", provider: "slack", mode: "normal", enabled: false });
    expect(proactive.connection).toMatchObject({ name: "社区 Slack", provider: "slack", mode: "proactive", enabled: false });
    expect(normal.connection.id).not.toBe(proactive.connection.id);
    expect(JSON.stringify(normal)).not.toContain("xapp-work");
    expect(JSON.stringify(normal)).not.toContain("xoxb-work");

    const list = await fetch(`http://127.0.0.1:${controlPort}/admin/api/im/connections`);
    expect(list.status).toBe(200);
    const listed = (await list.json()) as Record<string, any>;
    expect(listed.connections).toEqual(expect.arrayContaining([expect.objectContaining({ id: normal.connection.id, provider: "slack", mode: "normal" }), expect.objectContaining({ id: proactive.connection.id, provider: "slack", mode: "proactive" })]));

    const patch = await fetch(`http://127.0.0.1:${controlPort}/admin/api/im/connections/${encodeURIComponent(normal.connection.id)}`, {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: "工作区 Slack", mode: "proactive", enabled: true }),
    });
    expect(patch.status).toBe(200);
    await expect(patch.json()).resolves.toMatchObject({
      connection: { id: normal.connection.id, name: "工作区 Slack", mode: "proactive", enabled: true },
    });

    const saved = JSON.parse(await fs.readFile(path.join(tempRoot, "config.json"), "utf8")) as Record<string, any>;
    expect(saved.im_connections).toHaveLength(2);
    expect(saved.im_connections).toEqual(expect.arrayContaining([expect.objectContaining({ id: normal.connection.id, name: "工作区 Slack", mode: "proactive" }), expect.objectContaining({ id: proactive.connection.id, name: "社区 Slack", mode: "proactive" })]));

    const remove = await fetch(`http://127.0.0.1:${controlPort}/admin/api/im/connections/${encodeURIComponent(proactive.connection.id)}`, {
      method: "DELETE",
    });
    expect(remove.status).toBe(204);
  }, 60_000);
});
