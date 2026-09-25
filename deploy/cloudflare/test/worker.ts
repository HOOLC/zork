// Only the test bundler imports this entry. Production exports no fixture routes.
import worker, { Account as ProductionAccount, RelayHub as ProductionRelayHub, DiscoveryRecord, LoginAttempt, LoginLimiter } from "../src/index";
import { signToken, nowSeconds, reply, readJson } from "../src/auth";
import { budgetFor } from "../src/relay";
import type { Env } from "../src/env";
export { DiscoveryRecord, LoginAttempt, LoginLimiter };

export class Account extends ProductionAccount {
  expire(id: string, kind: "idle" | "absolute") {
    const account = this.ctx.storage.kv.get<any>("account");
    const session = account.sessions.find((s: any) => s.id === id);
    if (kind === "idle") session.idle = nowSeconds();
    else session.expires = nowSeconds();
    this.ctx.storage.kv.put("account", account);
  }
  statistics() {
    return {
      quota: this.ctx.storage.kv.get("quota"),
      account: this.ctx.storage.kv.get<any>("account")?.sessions.map((s: any) => ({
        id: s.id,
        generation: s.generation,
        expires: s.expires,
        idle: s.idle,
      })),
    };
  }
}

export class RelayHub extends ProductionRelayHub {
  exhaustBudget(kind: "bytes" | "frames" = "bytes", key = "global") {
    const now = nowSeconds();
    const limits = budgetFor(key);
    this.store(key, {
      minute: Math.floor(now / 60),
      day: Math.floor(now / 86400),
      at: now,
      connects: 0,
      bytes: kind === "bytes" ? limits.bytesPerDay : 0,
      frames: kind === "frames" ? limits.framesPerDay : 0,
      balance: 0,
      frameBalance: 0,
      savedBytes: 0,
      savedFrames: 0,
    });
  }
  /** Empties a rate bucket without touching the daily totals. */
  drainRate(key: string) {
    // Negative by one second of refill: refused for at least the next second.
    const q = this.quota(key);
    q.frameBalance = -budgetFor(key).framesPerSecond;
  }
  /** Makes pending handshakes look older (their start time lives in the attachment). */
  ageHandshakes(seconds: number) {
    for (const ws of this.ctx.getWebSockets()) {
      const att = ws.deserializeAttachment() as any;
      if (att && !att.ep) {
        att.since -= seconds;
        ws.serializeAttachment(att);
      }
    }
    this.forgetMemory();
  }
  statistics(key = "global") {
    return this.quota(key);
  }
  persisted(key = "global") {
    return this.ctx.storage.kv.get("quota:" + key) ?? null;
  }
  endpoints(ep: string) {
    return this.connections(ep).map(([, att]) => ({ seq: att.seq, sentTo: att.sentTo ?? [] }));
  }
  /** What hibernation does: drop every in-memory cache; sockets, tags, attachments and storage remain. */
  forgetMemory() {
    const fresh = new ProductionRelayHub(this.ctx, this.env) as any;
    for (const key of Object.keys(fresh)) (this as any)[key] = fresh[key];
  }
  debugSockets(tag?: string) {
    const list = tag === undefined ? this.ctx.getWebSockets() : this.ctx.getWebSockets(tag);
    return list.map((ws) => ({ state: ws.readyState, tags: this.ctx.getTags(ws) }));
  }
  sweep() {
    return this.alarm();
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/__test/devices") return reply(deviceRequests);
    if (path === "/__test/offline") {
      offline = !offline;
      return reply({ offline });
    }
    if (path === "/__test/lose-refresh-response") {
      loseRefreshResponse = true;
      return reply({ armed: true });
    }
    if (path === "/v1/auth/refresh" && loseRefreshResponse) {
      loseRefreshResponse = false;
      const response = await worker.fetch(request, env);
      return response.ok ? reply({ error: "lost_response" }, 503) : response;
    }
    if (offline && path.startsWith("/v1/auth/")) return reply({ error: "unavailable" }, 503);
    if (path === "/__test/access") {
      const body = await readJson(request);
      return reply({
        token: await signToken(
          env,
          "access",
          {
            sub: body.sub,
            sid: body.sid,
            email: "test@example.test",
          },
          nowSeconds() + Number(body.seconds),
        ),
      });
    }
    if (path === "/v1/auth/device" && request.method === "POST") {
      const response = await worker.fetch(request, env);
      if (response.ok) {
        deviceRequests.push(((await response.clone().json()) as { verification_uri: string }).verification_uri);
        if (deviceRequests.length > 8) deviceRequests.shift();
      }
      return response;
    }
    return worker.fetch(request, env);
  },
};
let offline = false;
let loseRefreshResponse = false;

const deviceRequests: string[] = [];
