// Only the test bundler imports this entry. Production exports no fixture routes.
import { DurableObject } from "cloudflare:workers";
import worker, { Account as ProductionAccount, RelayBudget as ProductionRelayBudget, DiscoveryRecord, LoginAttempt, LoginLimiter } from "../src/index";
import { signToken, nowSeconds, reply, readJson } from "../src/auth";
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

export class RelayBudget extends ProductionRelayBudget {
  exhaustBudget(kind: "bytes" | "frames" = "bytes") {
    const now = nowSeconds();
    this.ctx.storage.kv.put("quota", {
      minute: Math.floor(now / 60),
      day: Math.floor(now / 86400),
      at: now,
      requests: 0,
      connects: 0,
      bytes: kind === "bytes" ? 5 * 1024 * 1024 * 1024 : 0,
      frames: kind === "frames" ? 20_000_000 : 0,
      balance: 0,
    });
  }
  statistics() {
    return this.ctx.storage.kv.get("quota");
  }
}

export class Relay extends DurableObject<{ TEST_RELAY?: Fetcher }> {
  private delayMs = 0;
  private destroyed = 0;
  destroy() {
    this.destroyed++;
  }
  destroyCount() {
    return this.destroyed;
  }
  delay(milliseconds: number) {
    this.delayMs = milliseconds;
  }
  async fetch(request: Request): Promise<Response> {
    if (this.delayMs) await new Promise((resolve) => setTimeout(resolve, this.delayMs));
    if (request.headers.has("authorization") || request.headers.has("cookie") || new URL(request.url).search) {
      return new Response("credential leak", { status: 500 });
    }
    if (this.env.TEST_RELAY) {
      return this.env.TEST_RELAY.fetch(request);
    }
    const pair = new WebSocketPair();
    pair[1].binaryType = "arraybuffer";
    pair[1].accept();
    pair[1].addEventListener("message", (event) => pair[1].send(event.data));
    pair[1].addEventListener("close", () => pair[1].close(1000, "closed"));
    return new Response(null, {
      status: 101,
      webSocket: pair[0],
      headers: {
        "sec-websocket-protocol": request.headers.get("sec-websocket-protocol") ?? "iroh-relay",
      },
    });
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
